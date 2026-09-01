//! **The evaluator** — routes and source values in, per-destination totals out.
//!
//! One mechanism, two spellings, both here (ticket 0334):
//!
//! - [`eval_dests`] — **scalar**, one voice at a time, walking raw slots. The
//!   reference implementation: obvious, slow, and what the banked form is proved
//!   against.
//! - [`eval_dests_bank`] — **banked**, dest-major SoA and const-generic over the
//!   lane count. Routes on the outside, lanes on the inside, every per-route
//!   decision hoisted above the lane loop so what is left is a contiguous
//!   multiply-accumulate over `L` floats that LLVM contracts to NEON.
//!
//! Both synths ran their own copy of both until this ticket. vxn-1b's banked
//! form is the canon the shared one is transcribed from; vxn-2's lane-major
//! evaluator became transposable to it in
//! [0328](../../../../tickets/closed/0328-matrix-dest-major-lane-accumulators.md)
//! and route-compiled in
//! [0333](../../../../tickets/closed/0333-share-slot-and-route-compilation.md).
//!
//! ## Four things here are load-bearing and must not be tidied
//!
//! **The association.** The inner loop is `shaped · (gain · scale)`, grouped
//! that way deliberately: the scalar form multiplies by [`slot_gain`], which is
//! `topology · scale` already folded, so regrouping to `(shaped · gain) · scale`
//! rounds differently and costs the bit-exactness the two paths are held to.
//! [`crate::golden`]'s reassociation sweep is what notices; the exact-dyadic
//! case table cannot, because every grouping agrees on exact values.
//!
//! **Hoisting discipline.** Polarity, shape, scale-source polarity and scale
//! bend all dispatch *outside* the lane loop — fifteen straight-line arms (nine
//! polarity × shape, six fold × bend) rather than four decisions per lane.
//! Collapsing any one of them back inside costs the vectorisation, not a few
//! percent: hoisting `scale_norm`'s two decisions alone cut a fully-scaled
//! 16-slot eval by ~47% in vxn-2, and [[vxn2-matrix-hot-loop-lessons]] measured
//! a ~50% regression from exactly one such call riding in.
//!
//! **[`clamp_unit`] rather than `f32::clamp`**, and the branch inside
//! [`shape_log`](crate::curve::shape_log) rather than `copysign`. Both measured,
//! both counter-intuitive, both documented where they are defined
//! ([`crate::curve`]) — `f32::clamp`'s panic path costs ~7% in this loop and
//! `copysign` loses to the branch.
//!
//! **Slot order.** Destinations accumulate additively and float addition is not
//! associative, so "the same routes in the same order" is the whole contract
//! between the two paths. [`RouteList::compile`](crate::slot::RouteList::compile)
//! compacts stably for that reason.
//!
//! ## Storage, and why `R` is a parameter of functions that never read it
//!
//! Both evaluators are generic over a [`MatrixRoster`] they use for one thing:
//! the [`crate::storage`] width guards. Everything else a route needs —
//! the folded gain, the scale source's polarity — was resolved at compile time
//! and rides on the [`Route`]. The guard is not decoration: `NS` and `ND` are
//! inferred from the caller's arrays, so without it a 16-dest roster could be
//! handed a 51-wide accumulator and quietly use half of it.
//!
//! ## What is *not* here
//!
//! Anything that knows what a destination means. Applying a total to a filter
//! coefficient, a phase increment or a VCA stays in the synth, and so does every
//! coupling around the call: vxn-2's four deliberate one-block dest→source
//! feedback paths, its cross-stack lane-0 reduction for patch-global dests, its
//! `TargetFlags` gating, and vxn-1b's Amp factoring — which re-applies this
//! module's arithmetic piecewise through [`slot_topology_gain`] and
//! [`shape`](crate::curve::shape) rather than spelling it a second time.

use crate::curve::{
    Polarity, Shape, bend_exp, bend_lin, bend_log, clamp_unit, fold_bipolar, fold_unipolar,
    pol_abs, pol_bipolar, pol_direct, scale_norm, shape, shape_exp, shape_lin, shape_log,
};
use crate::roster::MatrixRoster;
use crate::slot::{DestEndpoint, MatrixSlot, Route, SourceEndpoint};
use crate::storage::{DestLanes, SourceLanes, assert_dest_width, assert_source_width, clear_dests};

// ── the slot's gain, in halves ──────────────────────────────────────────────

/// The **topology half** of a slot's gain: `cook_depth(depth) · gain(dest)`.
///
/// Depends only on the patch, so a consumer that resolves routes once per block
/// hoists it out of its per-voice loop —
/// [`RouteList::compile`](crate::slot::RouteList::compile) folds exactly this
/// into [`Route::gain`](crate::slot::Route::gain), and vxn-1b's Amp factoring
/// applies it on its own.
/// Spelled once so those three cannot drift.
///
/// Asks the destination for its gain rather than indexing a gain table, which is
/// equal for every real dest but not for the sentinel: a sentinel-free table
/// would fold `None` onto row 0 and quietly scale an unwired slot by the first
/// destination's gain. [`DestEndpoint::gain`] answers the identity there, which
/// is what a caller that forgot to check `is_active` would want.
#[inline]
pub fn slot_topology_gain<S, D: DestEndpoint>(slot: &MatrixSlot<S, D>) -> f32 {
    slot.dest.cook_depth(slot.depth) * slot.dest.gain()
}

/// The **per-voice half** of a slot's gain: its `scale_src` VCA resolved against
/// this voice's sources, or `1.0` for an unscaled slot.
///
/// The sentinel never reaches [`SourceEndpoint::is_bipolar`] — an unwired scale
/// source is the identity, decided on `idx()` alone.
#[inline]
pub fn slot_scale<S: SourceEndpoint, D, const NS: usize>(
    slot: &MatrixSlot<S, D>,
    sources: &[f32; NS],
) -> f32 {
    match slot.scale_src.idx() {
        Some(sc) => scale_norm(slot.scale_src.is_bipolar(), sources[sc], slot.scale_shape),
        None => 1.0,
    }
}

/// One slot's full gain — `cook_depth(depth) · dest_gain · scale_norm`, folded
/// into the single factor [`eval_dests`] multiplies by.
///
/// The association the banked form has to match: this is *one* number by the
/// time it reaches the accumulate, which is why [`eval_dests_bank`] groups
/// `shaped · (gain · scale)` and not `(shaped · gain) · scale`.
#[inline]
pub fn slot_gain<S: SourceEndpoint, D: DestEndpoint, const NS: usize>(
    slot: &MatrixSlot<S, D>,
    sources: &[f32; NS],
) -> f32 {
    slot_topology_gain(slot) * slot_scale(slot, sources)
}

// ── the scalar reference ────────────────────────────────────────────────────

/// Accumulate every active slot's contribution into one voice's per-dest totals.
/// Zeroes `out` first.
///
/// Takes **raw slots**, not compiled routes: it is the reference implementation,
/// and re-deriving per voice what [`RouteList::compile`](crate::slot::RouteList::compile)
/// resolves once per block is precisely what makes it the slow one. Switched-off,
/// unwired and zero-depth slots are skipped on the same predicate `compile`
/// drops on — [`MatrixSlot::is_active`] plus the zero-depth test — which is what
/// keeps the two paths bit-exact rather than merely close. vxn-1b's two
/// evaluators disagreed on exactly that predicate once (at `868faef`, where the
/// banked path honoured `enabled` and the scalar path did not); sharing the test
/// makes that class of bug unrepresentable.
///
/// `scale_src` is resolved from the same source table as the primary source, so
/// it can never form a cycle.
#[inline]
pub fn eval_dests<
    R: MatrixRoster,
    S: SourceEndpoint,
    D: DestEndpoint,
    const NS: usize,
    const ND: usize,
>(
    slots: &[MatrixSlot<S, D>],
    sources: &[f32; NS],
    out: &mut [f32; ND],
) {
    assert_source_width::<R, NS>();
    assert_dest_width::<R, ND>();
    out.fill(0.0);
    for slot in slots {
        // `is_active` is the switch *and* both endpoints — the same predicate
        // `RouteList::compile` drops on.
        if !slot.is_active() || slot.depth == 0.0 {
            continue;
        }
        let (Some(si), Some(di)) = (slot.source.idx(), slot.dest.idx()) else {
            continue;
        };
        out[di] += shape(slot.polarity, slot.shape, sources[si]) * slot_gain(slot, sources);
    }
}

// ── the banked form ─────────────────────────────────────────────────────────

/// [`eval_dests`] for a whole lane bank at once — the form both synths' render
/// loops run.
///
/// Identical arithmetic in an identical order, transposed: the outer loop is
/// routes, the inner loop is lanes, and every branch the scalar form takes per
/// lane (sentinel, off switch, zero depth, polarity, shape, scale source,
/// bipolar test, scale bend) has been hoisted above the inner loop or compiled
/// away by [`RouteList::compile`](crate::slot::RouteList::compile). What is left
/// is a contiguous multiply-accumulate over `L` contiguous floats.
///
/// The scatter goes too. `out[di] += …` with a runtime `di` serialises on a
/// store-to-load chain whenever two slots share a destination; here a route owns
/// its destination row for the whole inner loop.
///
/// Takes the compiled routes as a plain slice. A
/// [`RouteList`](crate::slot::RouteList) carries its own
/// `N`-wide array, so taking the list and unwrapping it *inside* would leave
/// LLVM a compile-time bound on the route loop that an opaque slice does not,
/// and a same-binary A/B said that was worth ~4%. Measured standalone — one
/// implementation per binary, which is how it ships — the list-taking spelling
/// was instead consistently **1% slower** on `matrix_bank_full` across three
/// interleaved rounds. The A/B was reading code layout, not the bound. Slice
/// kept: one fewer const parameter, and the faster of the two as shipped.
#[inline]
pub fn eval_dests_bank<
    R: MatrixRoster,
    const NS: usize,
    const ND: usize,
    const L: usize,
>(
    routes: &[Route],
    src: &SourceLanes<NS, L>,
    out: &mut DestLanes<ND, L>,
) {
    assert_source_width::<R, NS>();
    clear_dests::<R, ND, L>(out);
    // The per-route VCA, resolved for every lane before the accumulate. Kept
    // outside the route loop so it is written, not allocated, per route.
    let mut scale = [1.0f32; L];
    for r in routes {
        match r.scale {
            None => scale = [1.0; L],
            Some(sc) => {
                let sv = &src[sc as usize];
                // Fold and bend are both per-route constants, so both dispatch
                // here — six straight-line arms rather than a `scale_norm` call
                // carrying two branches into the lane loop. The arms are
                // `curve`'s free functions, which is what keeps this loop's
                // arithmetic and `scale_norm`'s the *same* arithmetic rather
                // than two spellings that agree today.
                macro_rules! vca_arm {
                    ($fold:path, $bend:path) => {
                        for l in 0..L {
                            scale[l] = $bend(clamp_unit($fold(sv[l])));
                        }
                    };
                }
                match (r.scale_bipolar, r.scale_shape) {
                    (false, Shape::Lin) => vca_arm!(fold_unipolar, bend_lin),
                    (false, Shape::Exp) => vca_arm!(fold_unipolar, bend_exp),
                    (false, Shape::Log) => vca_arm!(fold_unipolar, bend_log),
                    (true, Shape::Lin) => vca_arm!(fold_bipolar, bend_lin),
                    (true, Shape::Exp) => vca_arm!(fold_bipolar, bend_exp),
                    (true, Shape::Log) => vca_arm!(fold_bipolar, bend_log),
                }
            }
        }
        // Both rows are hoisted out of the lane loop: source-major `sv` and
        // dest-major `row` are each `L` contiguous floats, which is what lets
        // the accumulate vectorise.
        let sv = &src[r.src as usize];
        let row = &mut out[r.dest as usize];
        let g = r.gain;
        // `shaped · (gain · scale)` — the association matters. The scalar form
        // multiplies by `slot_gain`, which is `topology · scale` already folded,
        // so grouping the other way rounds differently and costs the
        // bit-exactness the golden harness asserts.
        macro_rules! curve_arm {
            ($pol:path, $bend:path) => {
                for l in 0..L {
                    row[l] += $bend($pol(sv[l])) * (g * scale[l]);
                }
            };
        }
        // Polarity × shape, dispatched once per route. Nine arms, each a
        // straight-line multiply-accumulate over `L` contiguous floats, built
        // from the same maps and bends `shape` dispatches on.
        match (r.polarity, r.shape) {
            (Polarity::Direct, Shape::Lin) => curve_arm!(pol_direct, shape_lin),
            (Polarity::Direct, Shape::Exp) => curve_arm!(pol_direct, shape_exp),
            (Polarity::Direct, Shape::Log) => curve_arm!(pol_direct, shape_log),
            (Polarity::Bipolar, Shape::Lin) => curve_arm!(pol_bipolar, shape_lin),
            (Polarity::Bipolar, Shape::Exp) => curve_arm!(pol_bipolar, shape_exp),
            (Polarity::Bipolar, Shape::Log) => curve_arm!(pol_bipolar, shape_log),
            (Polarity::Abs, Shape::Lin) => curve_arm!(pol_abs, shape_lin),
            (Polarity::Abs, Shape::Exp) => curve_arm!(pol_abs, shape_exp),
            (Polarity::Abs, Shape::Log) => curve_arm!(pol_abs, shape_log),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::slot::MatrixTable;

    /// Two sources. `Trap` panics if anything asks its polarity, which is how
    /// the "an unscaled slot never reads the sentinel" claim is checked rather
    /// than asserted.
    #[derive(Clone, Copy, Default)]
    enum Src {
        #[default]
        None,
        Real,
        Trap,
    }

    impl SourceEndpoint for Src {
        fn idx(self) -> Option<usize> {
            match self {
                Src::None => None,
                Src::Real => Some(0),
                Src::Trap => Some(1),
            }
        }
        fn is_bipolar(self) -> bool {
            match self {
                // The sentinel's polarity is never a question the evaluator has
                // an answer for: an unwired scale source is the identity, and
                // reaching this arm means something asked anyway.
                Src::None => panic!("the sentinel's polarity was read"),
                Src::Real => false,
                Src::Trap => true,
            }
        }
    }

    /// One plain destination and one carrying both a taper and a gain, so the
    /// fold order is observable.
    #[derive(Clone, Copy, Default)]
    enum Dst {
        #[default]
        None,
        Plain,
        Cooked,
    }

    impl DestEndpoint for Dst {
        fn idx(self) -> Option<usize> {
            match self {
                Dst::None => None,
                Dst::Plain => Some(0),
                Dst::Cooked => Some(1),
            }
        }
        fn gain(self) -> f32 {
            match self {
                Dst::Cooked => 12.0,
                _ => 1.0,
            }
        }
        fn cook_depth(self, depth: f32) -> f32 {
            match self {
                Dst::Cooked => depth * depth * depth,
                _ => depth,
            }
        }
    }

    fn wired(source: Src, dest: Dst, depth: f32) -> MatrixSlot<Src, Dst> {
        MatrixSlot {
            source,
            dest,
            depth,
            enabled: true,
            ..MatrixSlot::default()
        }
    }

    /// The taper runs on the raw depth and the gain runs after it — the order
    /// [`RouteList::compile`](crate::slot::RouteList::compile) folds into a
    /// route, and the order vxn-1b's Amp factoring re-applies. Backwards would
    /// give `(0.5 · 12)³`, six thousand times the answer.
    #[test]
    fn topology_gain_is_the_taper_then_the_native_unit() {
        assert_eq!(slot_topology_gain(&wired(Src::Real, Dst::Cooked, 0.5)), 0.125 * 12.0);
        assert_eq!(slot_topology_gain(&wired(Src::Real, Dst::Plain, 0.5)), 0.5);
    }

    /// `slot_gain` is exactly its two halves multiplied, in that order. This is
    /// the association [`eval_dests_bank`]'s `shaped · (gain · scale)` has to
    /// match: the scalar form multiplies by one already-folded number, so a bank
    /// that grouped `(shaped · gain) · scale` would round differently.
    #[test]
    fn slot_gain_is_the_two_halves_in_order() {
        let sources = [0.25f32, -0.5];
        let slot = MatrixSlot {
            scale_src: Src::Real,
            ..wired(Src::Real, Dst::Cooked, 0.5)
        };
        assert_eq!(
            slot_gain(&slot, &sources).to_bits(),
            (slot_topology_gain(&slot) * slot_scale(&slot, &sources)).to_bits()
        );
    }

    /// An unwired scale source is the identity, decided on `idx()` alone — it
    /// must never reach the source's polarity, which for a real roster panics
    /// out of range. `Src::None`'s `is_bipolar` panics so that a regression here
    /// fails rather than quietly folding a sentinel.
    #[test]
    fn an_unscaled_slot_is_unity_without_reading_a_polarity() {
        let slot = wired(Src::Real, Dst::Plain, 1.0);
        assert_eq!(slot_scale(&slot, &[0.25f32, -0.5]), 1.0);
    }

    /// The scalar form drops what the compile step drops — the off switch, both
    /// unwired endpoints, and zero depth — and leaves untouched destinations at
    /// exactly zero.
    #[test]
    fn the_scalar_form_drops_what_compile_drops() {
        let mut table: MatrixTable<Src, Dst, 4> = MatrixTable::default();
        table.slots[0] = MatrixSlot { enabled: false, ..wired(Src::Real, Dst::Plain, 1.0) };
        table.slots[1] = wired(Src::Real, Dst::None, 1.0);
        table.slots[2] = wired(Src::None, Dst::Plain, 1.0);
        table.slots[3] = wired(Src::Real, Dst::Plain, 0.0);

        let mut out = [f32::NAN; 2];
        eval_dests::<TwoByTwo, _, _, 2, 2>(&table.slots, &[1.0, 1.0], &mut out);
        assert_eq!(out, [0.0, 0.0]);
    }

    /// Slots into one destination **sum**, in slot order.
    #[test]
    fn slots_sharing_a_destination_accumulate() {
        let mut table: MatrixTable<Src, Dst, 4> = MatrixTable::default();
        table.slots[0] = wired(Src::Real, Dst::Plain, 0.5);
        table.slots[1] = wired(Src::Trap, Dst::Plain, 0.25);

        let mut out = [f32::NAN; 2];
        eval_dests::<TwoByTwo, _, _, 2, 2>(&table.slots, &[1.0, 2.0], &mut out);
        assert_eq!(out, [0.5 * 1.0 + 0.25 * 2.0, 0.0]);
    }

    /// A roster sized to the fixtures above, and nothing else — the widths are
    /// what the guards check, and the arithmetic columns are never read by these
    /// tests because the endpoints carry their own.
    #[derive(Clone, Copy)]
    struct TwoByTwo;

    impl MatrixRoster for TwoByTwo {
        const N_SOURCES: usize = 2;
        const N_DESTS: usize = 2;
        const N_SLOTS: usize = 4;

        fn source_is_bipolar(src: u8) -> bool {
            src == 1
        }
        fn dest_gain(dest: u8) -> f32 {
            if dest == 1 { 12.0 } else { 1.0 }
        }
        fn cook_depth(dest: u8, depth: f32) -> f32 {
            if dest == 1 { depth * depth * depth } else { depth }
        }
        fn dest_tier(_dest: u8) -> crate::roster::Tier {
            crate::roster::Tier::PerLane
        }
        fn source_tier(_src: u8) -> crate::roster::Tier {
            crate::roster::Tier::PerLane
        }
        fn dest_smoothing(_dest: u8) -> crate::roster::Smoothing {
            crate::roster::Smoothing::Block
        }
        fn source_names() -> &'static [&'static str] {
            &["a", "b"]
        }
        fn dest_names() -> &'static [&'static str] {
            &["x", "y"]
        }
        fn source_labels() -> &'static [&'static str] {
            &["A", "B"]
        }
        fn dest_labels() -> &'static [&'static str] {
            &["X", "Y"]
        }
    }

    /// The banked form is the scalar form transposed, and this is the smallest
    /// statement of that: same routes, same sources, `L` lanes each carrying its
    /// own value, bit-exact against `L` scalar evaluations. The exhaustive
    /// version is [`crate::golden`], which runs every case through both.
    #[test]
    fn the_banked_form_is_the_scalar_form_lane_by_lane() {
        const L: usize = 4;
        let mut table: MatrixTable<Src, Dst, 4> = MatrixTable::default();
        table.slots[0] = MatrixSlot {
            scale_src: Src::Trap,
            shape: Shape::Log,
            ..wired(Src::Real, Dst::Cooked, 0.5)
        };
        table.slots[1] = MatrixSlot {
            polarity: Polarity::Abs,
            ..wired(Src::Trap, Dst::Cooked, -0.75)
        };
        table.slots[2] = wired(Src::Real, Dst::Plain, 0.25);

        // Per-lane values that are not exactly representable, so a regrouping
        // shows up rather than cancelling.
        let lane_src: [[f32; 2]; L] =
            [[0.1, -0.3], [0.7, 0.2], [-0.9, 0.55], [1.0 / 3.0, -1.0 / 7.0]];

        let mut bank_src = [[0.0f32; L]; 2];
        for (l, vals) in lane_src.iter().enumerate() {
            for (s, &v) in vals.iter().enumerate() {
                bank_src[s][l] = v;
            }
        }
        let mut bank = [[f32::NAN; L]; 2];
        let routes = crate::slot::RouteList::compile(&table);
        eval_dests_bank::<TwoByTwo, 2, 2, L>(routes.active(), &bank_src, &mut bank);

        for (l, vals) in lane_src.iter().enumerate() {
            let mut one = [f32::NAN; 2];
            eval_dests::<TwoByTwo, _, _, 2, 2>(&table.slots, vals, &mut one);
            for d in 0..2 {
                assert_eq!(
                    bank[d][l].to_bits(),
                    one[d].to_bits(),
                    "lane {l} dest {d}: {} vs {}",
                    bank[d][l],
                    one[d]
                );
            }
        }
    }
}
