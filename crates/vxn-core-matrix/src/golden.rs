//! **Golden vectors**: routes and source values in, modulation amounts out.
//!
//! [ADR 0003](../../../../adrs/0003-vxn-core-matrix.md) §5 asks for one
//! declarative statement of what the matrix computes. A case is exactly that
//! and nothing more — *put these routes in at these depths, feed these source
//! values, get these modulation amounts* — run against
//! [`TestRoster`](crate::test_roster::TestRoster), whose gains are all 1.0 and
//! whose taper is the identity, so a number in an assertion is the evaluator's
//! arithmetic and nothing else.
//!
//! That is the difference from the tests this replaces. vxn-1b asserted
//! `out[Cutoff] == 24.0`, which is three claims at once — the evaluator
//! multiplies correctly, `DEST_GAIN[Cutoff]` is 48, and `Cutoff` takes no depth
//! taper — so changing a gain failed a test of the *evaluator*. Roster facts
//! stay tested per-synth, against the real roster; the mechanism is tested
//! here, once.
//!
//! ## The numbers are exact, on purpose
//!
//! Every source value in [`CASES`] is a dyadic rational chosen so that each
//! shaping arm lands on another one: `√0.25 = 0.5`, `0.25² = 0.0625`,
//! `2·0.625 − 1 = 0.25`. So every expectation is a short decimal that is
//! *exactly* representable, comparison is by bit pattern rather than epsilon,
//! and a reader can check a row by hand. A case that needed a tolerance would
//! be hiding which of the two — the arithmetic or the expectation — was
//! approximate.
//!
//! ## Four paths, and why the runner insists on more than one
//!
//! [`run_case`] evaluates every case through **every** path
//! [`eval_paths`] offers and requires them to agree **bit-exactly**. Float
//! addition is not associative, and "the same routes in the same order" is
//! already vxn-1b's stated contract between its scalar and banked evaluators;
//! this generalises that guarantee instead of re-deriving it per synth.
//!
//! Two of the paths are the harness's own, and are the two spellings this crate
//! documents in [`curve`](crate::curve): the **dispatchers** ([`shape`],
//! [`scale_norm`]), which is what a scalar one-value-at-a-time caller uses, and
//! the **free function arms** (`pol_abs` / `shape_log` / `fold_bipolar` /
//! `bend_exp`, …) expanded per route with every decision hoisted above the lane
//! loop. They differ in loop nesting, in whether routes are compacted before the
//! loop, and in how the nine polarity × shape and twelve fold × bend decisions
//! are dispatched — which is exactly where a reassociation would hide. The
//! harness's own banked path keeps the fused `(fold, bend)` spelling the shipped
//! one measured its way out of, which is the point: two spellings that must
//! agree bit-for-bit.
//!
//! The other two are [`crate::eval`]'s, registered by
//! [0334](../../../../tickets/closed/0334-share-the-evaluator.md): the scalar
//! per-voice reference and the banked lane loop, which is to say **the code both
//! synths ship**. The whole of [`CASES`] covers them without a line of new case
//! data, and the reassociation sweep covers the grouping the exact-dyadic table
//! cannot see. Keeping the harness's own pair alongside them is deliberate: the
//! shipped pair agreeing with itself proves a transposition preserved
//! arithmetic, while agreeing with a pair written independently of it is the
//! stronger claim.
//!
//! [`run_case`] **fails loudly when fewer than [`MIN_EVAL_PATHS`] paths exist**,
//! rather than reporting "all paths agree" about a set of one. If a later change
//! ever removes a path, the vacuous comparison is a failure, not a silent pass.
//!
//! ## The synths run the same table
//!
//! Since 0334 each synth's evaluator *is* one of the paths above, so the
//! coverage is direct. The per-synth bridges predate that and stay: each maps
//! the four synthetic sources and destinations onto four of its own that have
//! unit gain and no taper, runs [`CASES`] through its own `eval_dests`, and
//! checks the result against [`expected_totals`]. What they now assert is the
//! *binding* rather than the mechanism — that this synth's roster, widths and
//! endpoint enums reach the shared evaluator intact — which is the half a
//! roster-generic test cannot see. `vxn1b_engine::eval` and
//! `vxn2_engine::matrix` each have one.
//!
//! ## Adding a case
//!
//! Add a row to [`CASES`]. There is deliberately no test function per case —
//! the coverage list in
//! [0331](../../../../tickets/closed/0331-matrix-golden-vector-harness.md) is
//! data, and the run/compare/report loop is written once.
//!
//! A Rust `const` table rather than a TOML fixture: the values are floats
//! compared bit-exactly, and a text format would add a parse-and-round step
//! between the intention and the assertion.

use crate::curve::{
    Polarity, ScaleFold, Shape, bend_exp, bend_lin, bend_log, clamp_unit, curve_code, curve_split,
    fold_bipolar, fold_unipolar, pol_abs, pol_bipolar, pol_direct, scale_norm, shape, shape_exp,
    shape_lin, shape_log,
};
use crate::roster::MatrixRoster;
use crate::slot::{DestEndpoint, MatrixSlot, RouteList, SourceEndpoint};
use crate::storage::{DestLanes, SourceLanes, assert_source_width, clear_dests};

use core::marker::PhantomData;

use Polarity::{Abs, Bipolar, Direct};
use Shape::{Exp, Lin, Log};

// ── the case vocabulary ─────────────────────────────────────────────────────

/// The unwired-endpoint sentinel, in the case table's own spelling.
///
/// Both synths spell "empty slot" as discriminant 0 of their own enum and drop
/// it at the seam ([`MatrixRoster`]'s indices are sentinel-free), so a case
/// table needs its own way to say *unwired*. `u8::MAX` cannot collide with a
/// storage index of any roster the mechanism could plausibly serve, and
/// [`run_case`] range-checks every index that is not this, so a typo in a row
/// is a loud failure rather than a silently different route.
pub const NONE: u8 = u8::MAX;

/// [`TestRoster`](crate::test_roster::TestRoster)'s first bipolar source.
pub const BI_A: u8 = 0;
/// Its second bipolar source — the one the cases use as a scale source, so a
/// row can drive the route and the VCA from different values.
pub const BI_B: u8 = 1;
/// Its first unipolar source.
pub const UNI_A: u8 = 2;
/// Its second unipolar source, likewise the unipolar scale source.
pub const UNI_B: u8 = 3;

/// [`TestRoster`](crate::test_roster::TestRoster)'s four destinations. They
/// differ in tier and smoothing class but not in gain or taper, so which one a
/// case routes to never changes a number.
pub const DEST_A: u8 = 0;
/// See [`DEST_A`].
pub const DEST_B: u8 = 1;
/// See [`DEST_A`].
pub const DEST_C: u8 = 2;
/// See [`DEST_A`].
pub const DEST_D: u8 = 3;

/// A route's on/off switch, switched on — spelled out so a row's last column
/// reads as a word rather than a bare `true`.
pub const ON: bool = true;
/// A route's on/off switch, switched off.
pub const OFF: bool = false;

/// Scale curve: fold by the source's own polarity, no bend. The `scale_curve`
/// column is a raw byte decoded with [`curve_split`], the same flat
/// `(polarity, shape)` code as the primary axis, so a case can also spell a
/// byte past the table.
///
/// The three `BEND_*` spellings are `Direct` on the scale polarity, which is
/// the pre-0341 VCA — hence `curve_code(Direct, shape) == shape as u8`, and
/// hence every case row written before the axis existed means what it always
/// meant.
pub const BEND_LIN: u8 = curve_code(Direct, Lin);
/// Scale curve: fold, then square.
pub const BEND_EXP: u8 = curve_code(Direct, Exp);
/// Scale curve: fold, then root.
pub const BEND_LOG: u8 = curve_code(Direct, Log);
/// Scale curve: rectify (`|v|`), no bend — the gate open at **both** extremes
/// of a bipolar source, shut at its centre.
pub const SCALE_ABS: u8 = curve_code(Abs, Lin);
/// Scale curve: rectify, then square.
pub const SCALE_ABS_EXP: u8 = curve_code(Abs, Exp);
/// Scale curve: rectify, then root.
pub const SCALE_ABS_LOG: u8 = curve_code(Abs, Log);
/// Scale curve: AC-couple (`2v − 1`), no bend — the threshold-ish gate over the
/// source's upper half.
pub const SCALE_BIPOLAR: u8 = curve_code(Bipolar, Lin);
/// Scale curve: AC-couple, then square.
pub const SCALE_BIPOLAR_EXP: u8 = curve_code(Bipolar, Exp);
/// Scale curve: AC-couple, then root.
pub const SCALE_BIPOLAR_LOG: u8 = curve_code(Bipolar, Log);

/// One routing in a case, in the shape a slot reaches the evaluator in.
///
/// The shaping is carried as the **flat curve code** rather than as two typed
/// axes. That costs a row nothing — [`curve_code`] is a `const fn`, so a row
/// spells `curve_code(Abs, Lin)` and reads as the pair — and it buys the
/// coverage item a typed field could not express: a code past the table must
/// degrade to `(Direct, Lin)` rather than alias onto a real curve, which is
/// what a corrupt preset does. `scale_curve` is a raw byte for the same reason.
#[derive(Clone, Copy, Debug)]
pub struct Route {
    /// Source storage index, or [`NONE`] for an unwired route.
    pub source: u8,
    /// Destination storage index, or [`NONE`].
    pub dest: u8,
    /// The slot's raw, un-tapered depth. The roster's `cook_depth` runs on it.
    pub depth: f32,
    /// Flat `(polarity, shape)` code — see [`curve_code`] / [`curve_split`].
    pub curve: u8,
    /// The per-route VCA's source index, or [`NONE`] for an unscaled route.
    pub scale_src: u8,
    /// The VCA's own flat `(polarity, shape)` code — the same nine the primary
    /// axis has, since 0341 — as a raw byte.
    pub scale_curve: u8,
    /// The player's on/off switch. A switched-off route keeps its wiring and
    /// contributes nothing.
    pub enabled: bool,
}

/// Build a [`Route`]. A `const fn` so a case row is one call rather than a
/// seven-field struct literal.
pub const fn route(
    source: u8,
    dest: u8,
    depth: f32,
    curve: u8,
    scale_src: u8,
    scale_curve: u8,
    enabled: bool,
) -> Route {
    Route {
        source,
        dest,
        depth,
        curve,
        scale_src,
        scale_curve,
        enabled,
    }
}

impl Route {
    /// Whether this route reaches the accumulator at all: switched on, both
    /// endpoints wired, depth not zero.
    ///
    /// Deliberately shared by both evaluator paths. This predicate *is* the
    /// case's semantics — vxn-1b's two evaluators share it too (`is_active()`
    /// plus the zero-depth skip), and it is what makes a disabled or inert
    /// slot droppable at compile time in one path and skippable inline in the
    /// other. What the paths must not share is the arithmetic, and they don't.
    #[inline]
    fn is_live(&self) -> bool {
        self.enabled && self.source != NONE && self.dest != NONE && self.depth != 0.0
    }

    /// The two shaping axes this route's flat code stands for.
    #[inline]
    fn axes(&self) -> (Polarity, Shape) {
        curve_split(self.curve)
    }

    /// The two VCA axes this route's raw scale byte stands for.
    #[inline]
    fn scale_axes(&self) -> (Polarity, Shape) {
        curve_split(self.scale_curve)
    }
}

/// One golden vector: routes in, source values in, modulation amounts out.
///
/// A source not named in `sources` reads 0.0; a destination not named in
/// `expect` must come out **exactly** 0.0, which is what makes "this route
/// wrote only its own destination" a property of every row rather than a case
/// of its own.
#[derive(Clone, Copy, Debug)]
pub struct Case {
    /// What this row is for. Appears in every failure message, so it should
    /// name the claim, not the setup.
    pub name: &'static str,
    /// The matrix table, in slot order. Order is load-bearing: destinations
    /// accumulate additively and float addition is not associative.
    pub routes: &'static [Route],
    /// `(source storage index, value)`. Broadcast across every lane.
    pub sources: &'static [(u8, f32)],
    /// `(destination storage index, expected total)`. Every other destination
    /// must be 0.0.
    pub expect: &'static [(u8, f32)],
}

// ── the evaluator paths ─────────────────────────────────────────────────────

/// The number of evaluator paths below which [`run_case`]'s cross-path
/// agreement claim is vacuous, and so a failure rather than a pass.
///
/// One path cannot disagree with itself. A runner that reported success in
/// that state would be testing half of what it claims — silently, and at
/// exactly the moment a path had been lost.
pub const MIN_EVAL_PATHS: usize = 2;

/// One way of turning routes and per-lane source values into a destination
/// accumulator.
///
/// A plain function pointer rather than a trait object: the widths are const
/// generic parameters, and monomorphising the arms into a `fn` keeps the list a
/// runtime value that [`run_case`] can count and name.
#[derive(Clone, Copy)]
pub struct EvalPath<const NS: usize, const ND: usize, const L: usize> {
    /// What distinguishes this path from the others, for failure messages.
    pub name: &'static str,
    /// Zeroes `out`, then accumulates every live route into it.
    pub eval: fn(&[Route], &SourceLanes<NS, L>, &mut DestLanes<ND, L>),
}

/// Every evaluator path this crate offers for roster `R`, at these widths.
///
/// **Four**, in two pairs. The first two are the harness's own reference
/// spellings — the [`curve`](crate::curve) dispatchers, and the hoisted free
/// function arms — written independently of anything that ships, which is what
/// makes them worth comparing against. The second two are what *does* ship,
/// registered by 0334: [`crate::eval`]'s scalar and banked forms, the ones both
/// synths now run. The whole of [`CASES`] covers them without a line of new case
/// data, and the reassociation sweep covers the grouping the case table cannot
/// see.
///
/// Two pairs rather than one is the point. The shared pair agreeing with itself
/// would prove only that a transposition preserved arithmetic; agreeing with a
/// pair nobody wrote with the shipped evaluator in view is the stronger claim.
pub fn eval_paths<R: MatrixRoster, const NS: usize, const ND: usize, const L: usize>()
-> Vec<EvalPath<NS, ND, L>> {
    vec![
        EvalPath {
            name: "scalar/dispatched",
            eval: eval_scalar::<R, NS, ND, L>,
        },
        EvalPath {
            name: "banked/hoisted",
            eval: eval_banked::<R, NS, ND, L>,
        },
        EvalPath {
            name: "shared/scalar",
            eval: eval_shared_scalar::<R, NS, ND, L>,
        },
        EvalPath {
            name: "shared/banked",
            eval: eval_shared_bank::<R, NS, ND, L>,
        },
    ]
}

// ── the shipped evaluator, as two more paths ────────────────────────────────

/// A [`SourceEndpoint`] over a roster's bare storage index, so a case row can be
/// turned into the [`MatrixSlot`] the shipped evaluator takes.
///
/// A synth crosses this seam with its own `SourceId`; a case row has only a
/// number and [`NONE`], so it needs an endpoint type of its own. Everything
/// keyed on the source is asked of `R`, which is what keeps this an adapter
/// rather than a third roster.
///
/// `is_bipolar` forwards to `R` **without** guarding the sentinel, deliberately:
/// a roster's lookups panic out of range by contract
/// ([`crate::roster::MatrixRoster`]), so this adapter turns "something read an
/// unwired scale source's polarity" into a loud failure on cases that have one.
/// [`crate::slot::compile_slots`] did exactly that until 0334 and nothing
/// noticed, because both synths' generated enums happen to answer `false` for
/// their own sentinel.
#[derive(Debug)]
pub struct RosterSource<R>(u8, PhantomData<R>);

/// A [`DestEndpoint`] over a roster's bare storage index — the destination twin
/// of [`RosterSource`].
///
/// `gain` and `cook_depth` are the identity at the sentinel for the same reason
/// the synths' generated enums make them so: an unwired slot is dropped before
/// either is read, and the identity is the answer that cannot mislead if one
/// ever is.
#[derive(Debug)]
pub struct RosterDest<R>(u8, PhantomData<R>);

impl<R> Clone for RosterSource<R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<R> Copy for RosterSource<R> {}
impl<R> Clone for RosterDest<R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<R> Copy for RosterDest<R> {}

impl<R: MatrixRoster> SourceEndpoint for RosterSource<R> {
    #[inline]
    fn idx(self) -> Option<usize> {
        (self.0 != NONE).then_some(self.0 as usize)
    }
    #[inline]
    fn is_bipolar(self) -> bool {
        R::source_is_bipolar(self.0)
    }
}

impl<R: MatrixRoster> DestEndpoint for RosterDest<R> {
    #[inline]
    fn idx(self) -> Option<usize> {
        (self.0 != NONE).then_some(self.0 as usize)
    }
    #[inline]
    fn gain(self) -> f32 {
        if self.0 == NONE { 1.0 } else { R::dest_gain(self.0) }
    }
    #[inline]
    fn cook_depth(self, depth: f32) -> f32 {
        if self.0 == NONE {
            depth
        } else {
            R::cook_depth(self.0, depth)
        }
    }
}

/// Width of the [`RouteList`] the banked path compiles a case into.
///
/// The path could hand [`crate::eval::eval_dests_bank`] a bare `Vec<Route>` and
/// skip this, but then the real list type — the one both synths hold, sized and
/// allocation-free — would be the one thing in the pipeline the case table did
/// not cover. So the harness names a width instead. `R::N_SLOTS` will not serve:
/// it is an associated const, and stable Rust does not accept one as an array
/// length. 16 is both synths' table width, and at least as wide as any roster
/// the runner admits (`check_and_expand` rejects a case with more routes than
/// `R::N_SLOTS`), so nothing can compile past it — and
/// [`RouteList::from_slots`] asserts rather than truncating if something ever
/// does.
const SLOT_WIDTH: usize = 16;

/// Case rows as the slots a patch would hold — the input both shipped paths
/// take, so neither gets a differently-prepared table.
fn as_slots<R: MatrixRoster>(routes: &[Route]) -> Vec<MatrixSlot<RosterSource<R>, RosterDest<R>>> {
    routes
        .iter()
        .map(|r| {
            let (polarity, shape) = r.axes();
            let (scale_polarity, scale_shape) = r.scale_axes();
            MatrixSlot {
                source: RosterSource(r.source, PhantomData),
                dest: RosterDest(r.dest, PhantomData),
                depth: r.depth,
                polarity,
                shape,
                enabled: r.enabled,
                scale_src: RosterSource(r.scale_src, PhantomData),
                scale_polarity,
                scale_shape,
            }
        })
        .collect()
}

/// Path 3 — the shipped scalar evaluator, [`crate::eval::eval_dests`], run once
/// per lane.
///
/// One lane at a time is what that function is: a per-voice reference walking
/// raw slots. Running it `L` times and transposing the results back is therefore
/// also a cross-lane-leakage check on every *other* path — lane `l` of a bank
/// has to equal a one-voice evaluation on lane `l`'s inputs, and here that is
/// enforced on every case rather than in a test of its own.
fn eval_shared_scalar<R: MatrixRoster, const NS: usize, const ND: usize, const L: usize>(
    routes: &[Route],
    src: &SourceLanes<NS, L>,
    out: &mut DestLanes<ND, L>,
) {
    let slots = as_slots::<R>(routes);
    for lane in 0..L {
        let mut one = [0.0f32; NS];
        for (s, row) in src.iter().enumerate() {
            one[s] = row[lane];
        }
        let mut totals = [0.0f32; ND];
        crate::eval::eval_dests::<R, _, _, NS, ND>(&slots, &one, &mut totals);
        for (d, row) in out.iter_mut().enumerate() {
            row[lane] = totals[d];
        }
    }
}

/// Path 4 — the shipped banked evaluator,
/// [`crate::eval::eval_dests_bank`], over routes resolved by the shipped
/// [`RouteList::from_slots`].
///
/// Compiles through the real compile step rather than a harness copy of it, so
/// the drop predicate and the `cook_depth · dest_gain` fold are under test here
/// too, not just the lane loop.
fn eval_shared_bank<R: MatrixRoster, const NS: usize, const ND: usize, const L: usize>(
    routes: &[Route],
    src: &SourceLanes<NS, L>,
    out: &mut DestLanes<ND, L>,
) {
    let slots = as_slots::<R>(routes);
    let compiled = RouteList::<SLOT_WIDTH>::from_slots(&slots);
    crate::eval::eval_dests_bank::<R, NS, ND, L>(compiled.active(), src, out);
}

/// Path 1 — one lane at a time, every decision taken inside the loop through
/// the [`curve`](crate::curve) dispatchers.
///
/// This is the shape of vxn-1b's per-voice `eval_dests`: no route compilation,
/// the sentinel and zero-depth checks re-run per lane, and `shape` /
/// `scale_norm` called as functions. Slow, obvious, and the reference the
/// hoisted form has to match.
fn eval_scalar<R: MatrixRoster, const NS: usize, const ND: usize, const L: usize>(
    routes: &[Route],
    src: &SourceLanes<NS, L>,
    out: &mut DestLanes<ND, L>,
) {
    assert_source_width::<R, NS>();
    clear_dests::<R, ND, L>(out);
    for lane in 0..L {
        for r in routes {
            if !r.is_live() {
                continue;
            }
            let gain = R::cook_depth(r.dest, r.depth) * R::dest_gain(r.dest);
            let vca = if r.scale_src == NONE {
                1.0
            } else {
                let (scale_polarity, scale_bend) = r.scale_axes();
                scale_norm(
                    R::source_is_bipolar(r.scale_src),
                    src[r.scale_src as usize][lane],
                    scale_polarity,
                    scale_bend,
                )
            };
            let (polarity, bend) = r.axes();
            out[r.dest as usize][lane] +=
                shape(polarity, bend, src[r.source as usize][lane]) * (gain * vca);
        }
    }
}

/// One live route with its lane-invariant half resolved — the compacted form
/// [`eval_banked`] walks.
#[derive(Clone, Copy)]
struct Compiled {
    src: u8,
    dest: u8,
    polarity: Polarity,
    shape: Shape,
    /// `cook_depth(depth) · dest_gain`, hoisted out of the lane loop.
    gain: f32,
    scale: u8,
    scale_fold: ScaleFold,
    scale_bend: Shape,
}

/// Path 2 — routes compiled once, then a straight-line lane loop per route.
///
/// This is the shape of vxn-1b's `eval_dests_bank`: inert routes dropped by
/// compaction rather than branched over per lane, the taper and gain resolved
/// once, and all twenty-one per-route decisions (nine polarity × shape, twelve
/// fold × bend) dispatched *above* the lane loop into the free-function arms.
///
/// The fused `(fold, bend)` match here is deliberately **not** the shipped
/// loop's split pair — see [`crate::eval::eval_dests_bank`], which measured its
/// way to two dispatches. Same arithmetic, different spelling, and the runner
/// requires them bit-exact.
///
/// The association `shaped · (gain · vca)` follows [`eval_scalar`], which
/// multiplies by one already-folded factor — grouping the other way rounds
/// differently on values that are not exactly representable, which is the
/// difference the two paths exist to catch. [`CASES`] cannot see it (its values
/// are chosen exact, so every grouping agrees); `paths_agree_on_values_that_do_not_round_exactly`
/// can, and does. vxn-2's shipped loop grouped the other way
/// (`shaped · depth · scale`) until 0333 gave it a compiled
/// [`crate::slot::Route`] carrying one pre-folded factor; both synths and this
/// harness now group the same way,
/// so [0334](../../../../tickets/closed/0334-share-the-evaluator.md) inherits one
/// association rather than reconciling two.
fn eval_banked<R: MatrixRoster, const NS: usize, const ND: usize, const L: usize>(
    routes: &[Route],
    src: &SourceLanes<NS, L>,
    out: &mut DestLanes<ND, L>,
) {
    assert_source_width::<R, NS>();
    clear_dests::<R, ND, L>(out);
    let compiled: Vec<Compiled> = routes
        .iter()
        .filter(|r| r.is_live())
        .map(|r| {
            let (polarity, shape) = r.axes();
            Compiled {
                src: r.source,
                dest: r.dest,
                polarity,
                shape,
                gain: R::cook_depth(r.dest, r.depth) * R::dest_gain(r.dest),
                scale: r.scale_src,
                scale_fold: ScaleFold::resolve(
                    r.scale_axes().0,
                    r.scale_src != NONE && R::source_is_bipolar(r.scale_src),
                ),
                scale_bend: r.scale_axes().1,
            }
        })
        .collect();

    // Written, not allocated, per route — the same reason both synths keep it
    // outside their route loop.
    let mut vca = [1.0f32; L];
    for c in &compiled {
        if c.scale == NONE {
            vca = [1.0; L];
        } else {
            let sv = &src[c.scale as usize];
            macro_rules! vca_arm {
                ($fold:path, $bend:path) => {
                    for l in 0..L {
                        vca[l] = $bend(clamp_unit($fold(sv[l])));
                    }
                };
            }
            match (c.scale_fold, c.scale_bend) {
                (ScaleFold::Passthrough, Lin) => vca_arm!(fold_unipolar, bend_lin),
                (ScaleFold::Passthrough, Exp) => vca_arm!(fold_unipolar, bend_exp),
                (ScaleFold::Passthrough, Log) => vca_arm!(fold_unipolar, bend_log),
                (ScaleFold::Fold, Lin) => vca_arm!(fold_bipolar, bend_lin),
                (ScaleFold::Fold, Exp) => vca_arm!(fold_bipolar, bend_exp),
                (ScaleFold::Fold, Log) => vca_arm!(fold_bipolar, bend_log),
                (ScaleFold::Rectify, Lin) => vca_arm!(pol_abs, bend_lin),
                (ScaleFold::Rectify, Exp) => vca_arm!(pol_abs, bend_exp),
                (ScaleFold::Rectify, Log) => vca_arm!(pol_abs, bend_log),
                (ScaleFold::AcCouple, Lin) => vca_arm!(pol_bipolar, bend_lin),
                (ScaleFold::AcCouple, Exp) => vca_arm!(pol_bipolar, bend_exp),
                (ScaleFold::AcCouple, Log) => vca_arm!(pol_bipolar, bend_log),
            }
        }
        let pv = &src[c.src as usize];
        let row = &mut out[c.dest as usize];
        let g = c.gain;
        macro_rules! curve_arm {
            ($pol:path, $bend:path) => {
                for l in 0..L {
                    row[l] += $bend($pol(pv[l])) * (g * vca[l]);
                }
            };
        }
        match (c.polarity, c.shape) {
            (Direct, Lin) => curve_arm!(pol_direct, shape_lin),
            (Direct, Exp) => curve_arm!(pol_direct, shape_exp),
            (Direct, Log) => curve_arm!(pol_direct, shape_log),
            (Bipolar, Lin) => curve_arm!(pol_bipolar, shape_lin),
            (Bipolar, Exp) => curve_arm!(pol_bipolar, shape_exp),
            (Bipolar, Log) => curve_arm!(pol_bipolar, shape_log),
            (Abs, Lin) => curve_arm!(pol_abs, shape_lin),
            (Abs, Exp) => curve_arm!(pol_abs, shape_exp),
            (Abs, Log) => curve_arm!(pol_abs, shape_log),
        }
    }
}

// ── the runner ──────────────────────────────────────────────────────────────

/// Run one case through every evaluator path and check the lot.
///
/// Three assertions, in the order a failure is easiest to read: the case is
/// well-formed, every path agrees with every other bit-exactly, and the agreed
/// result is what the case says it should be — on **every** lane and for
/// **every** destination, including the ones the case does not mention, which
/// must be exactly zero.
///
/// Comparison is by bit pattern throughout, so a `-0.0` for `0.0` or a NaN
/// cannot slip past.
///
/// Panics — loudly, and naming the count — if fewer than [`MIN_EVAL_PATHS`]
/// paths exist, because "they all agree" is not a claim worth making about a
/// set of one.
pub fn run_case<R: MatrixRoster, const NS: usize, const ND: usize, const L: usize>(case: &Case) {
    let expected = check_and_expand::<R, ND>(case);

    let paths = eval_paths::<R, NS, ND, L>();
    assert!(
        paths.len() >= MIN_EVAL_PATHS,
        "'{}': the golden runner asserts every evaluator path agrees, but only {} \
         {} available (need at least {MIN_EVAL_PATHS}). Comparing a path with itself \
         proves nothing — register the missing path or delete the claim.",
        case.name,
        paths.len(),
        if paths.len() == 1 { "is" } else { "are" },
    );

    let mut src: SourceLanes<NS, L> = [[0.0; L]; NS];
    for &(si, v) in case.sources {
        src[si as usize] = [v; L];
    }

    let results: Vec<DestLanes<ND, L>> = paths
        .iter()
        .map(|p| {
            let mut out: DestLanes<ND, L> = [[f32::NAN; L]; ND];
            (p.eval)(case.routes, &src, &mut out);
            out
        })
        .collect();

    // Every path against the first. Transitivity does the rest, and pinning one
    // reference keeps the message short enough to read.
    for (p, got) in paths.iter().zip(&results).skip(1) {
        for d in 0..ND {
            for l in 0..L {
                assert_eq!(
                    got[d][l].to_bits(),
                    results[0][d][l].to_bits(),
                    "'{}' L={L}: path '{}' disagrees with '{}' at dest {d} lane {l}: \
                     {} vs {}",
                    case.name,
                    p.name,
                    paths[0].name,
                    got[d][l],
                    results[0][d][l],
                );
            }
        }
    }

    for d in 0..ND {
        for l in 0..L {
            assert_eq!(
                results[0][d][l].to_bits(),
                expected[d].to_bits(),
                "'{}' L={L}: dest {d} lane {l} is {}, expected {}",
                case.name,
                results[0][d][l],
                expected[d],
            );
        }
    }
}

/// Validate a case against roster `R` and expand its sparse `expect` list into
/// a dense per-destination array, unmentioned destinations reading 0.0.
///
/// Public because a **synth** wants it. A case is written in
/// [`TestRoster`](crate::test_roster::TestRoster)'s index space, but a synth
/// can map those four sources and four destinations onto four of its own with
/// unit gain and no taper, run its *shipped* evaluator over the same routes,
/// and check the result against this — which is how vxn-1b's and vxn-2's own
/// route-shaping arms get covered by this table before
/// [0334](../../../../tickets/closed/0334-share-the-evaluator.md) makes them one
/// evaluator. Without that bridge the table would only ever prove the harness
/// consistent with itself.
///
/// The validation is not ceremony: every index in a case row is a bare number,
/// and a mistyped one would otherwise reach the roster, whose lookups panic
/// with a message about the roster rather than about the case that broke.
pub fn expected_totals<R: MatrixRoster, const ND: usize>(case: &Case) -> [f32; ND] {
    check_and_expand::<R, ND>(case)
}

fn check_and_expand<R: MatrixRoster, const ND: usize>(case: &Case) -> [f32; ND] {
    let name = case.name;
    assert!(
        case.routes.len() <= R::N_SLOTS,
        "'{name}': {} routes exceeds the roster's {} slots",
        case.routes.len(),
        R::N_SLOTS
    );

    let source_ok = |i: u8| i == NONE || (i as usize) < R::N_SOURCES;
    for (n, r) in case.routes.iter().enumerate() {
        assert!(
            source_ok(r.source),
            "'{name}': route {n} source {} is past the roster",
            r.source
        );
        assert!(
            source_ok(r.scale_src),
            "'{name}': route {n} scale source {} is past the roster",
            r.scale_src
        );
        assert!(
            r.dest == NONE || (r.dest as usize) < R::N_DESTS,
            "'{name}': route {n} dest {} is past the roster",
            r.dest
        );
    }

    let mut seen_source = [false; 256];
    for &(si, _) in case.sources {
        assert!(
            (si as usize) < R::N_SOURCES,
            "'{name}': source index {si} is past the roster"
        );
        assert!(
            !seen_source[si as usize],
            "'{name}': source {si} is given twice"
        );
        seen_source[si as usize] = true;
    }

    let mut expected = [0.0f32; ND];
    let mut seen_dest = [false; 256];
    for &(di, want) in case.expect {
        assert!(
            (di as usize) < ND,
            "'{name}': expected dest {di} is past the roster"
        );
        assert!(!seen_dest[di as usize], "'{name}': dest {di} is expected twice");
        seen_dest[di as usize] = true;
        expected[di as usize] = want;
    }
    expected
}

// ── the cases ───────────────────────────────────────────────────────────────

/// Every golden vector, one row per case.
///
/// Written against [`TestRoster`](crate::test_roster::TestRoster)'s four
/// sources and four destinations. Because every gain is 1.0 and every taper is
/// the identity, a route's contribution is exactly
/// `shape(polarity, bend, source) · (depth · vca)` and the expectation column
/// is arithmetic a reader can redo.
///
/// The groups follow
/// [0331](../../../../tickets/closed/0331-matrix-golden-vector-harness.md)'s
/// coverage list. `polarity_shape_pairs_are_all_covered` and
/// `scale_polarities_and_bends_are_all_covered` hold the first two groups to
/// it, so a row deleted in a later edit is a failure rather than a quiet gap.
pub const CASES: &[Case] = &[
    // ── all nine polarity × shape pairs ─────────────────────────────────────
    //
    // `bi-a` reads −0.25 and `uni-a` reads 0.625, chosen so that the bipolar
    // map lands on ∓0.25 too: every arm then produces 0.25, 0.0625 or 0.5,
    // exactly.
    Case {
        name: "direct/lin passes the source straight through",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, -0.25)],
        expect: &[(DEST_A, -0.25)],
    },
    Case {
        name: "direct/exp is the signed square",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Exp), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, -0.25)],
        expect: &[(DEST_A, -0.0625)],
    },
    Case {
        name: "direct/log is the signed root",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Log), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, -0.25)],
        expect: &[(DEST_A, -0.5)],
    },
    Case {
        name: "bipolar/lin AC-couples a unipolar source",
        routes: &[route(UNI_A, DEST_A, 1.0, curve_code(Bipolar, Lin), NONE, BEND_LIN, ON)],
        sources: &[(UNI_A, 0.625)],
        expect: &[(DEST_A, 0.25)],
    },
    Case {
        // Polarity runs first: the bend squares the *mapped* 0.25. Shape-first
        // would square 0.625 and then map, giving −0.21875.
        name: "bipolar/exp bends the mapped value, not the raw one",
        routes: &[route(UNI_A, DEST_A, 1.0, curve_code(Bipolar, Exp), NONE, BEND_LIN, ON)],
        sources: &[(UNI_A, 0.625)],
        expect: &[(DEST_A, 0.0625)],
    },
    Case {
        name: "bipolar/log roots the mapped value",
        routes: &[route(UNI_A, DEST_A, 1.0, curve_code(Bipolar, Log), NONE, BEND_LIN, ON)],
        sources: &[(UNI_A, 0.625)],
        expect: &[(DEST_A, 0.5)],
    },
    Case {
        name: "abs/lin rectifies a bipolar source",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Abs, Lin), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, -0.25)],
        expect: &[(DEST_A, 0.25)],
    },
    Case {
        name: "abs/exp rectifies then squares",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Abs, Exp), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, -0.25)],
        expect: &[(DEST_A, 0.0625)],
    },
    Case {
        name: "abs/log rectifies then roots",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Abs, Log), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, -0.25)],
        expect: &[(DEST_A, 0.5)],
    },
    // The rest of `abs`'s claim: both extremes drive the dest the same way, the
    // centre not at all, and a negative depth mirrors the whole thing rather
    // than needing a second curve. This is the spread→pan route.
    Case {
        name: "abs treats the positive extreme like the negative one",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Abs, Lin), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, 0.25)],
        expect: &[(DEST_A, 0.25)],
    },
    Case {
        name: "abs leaves a centred source alone",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Abs, Lin), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, 0.0)],
        expect: &[],
    },
    Case {
        name: "abs mirrors under a negative depth",
        routes: &[route(BI_A, DEST_A, -1.0, curve_code(Abs, Lin), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, -0.25)],
        expect: &[(DEST_A, -0.25)],
    },
    // ── both scale-source polarities × all three scale bends ────────────────
    //
    // The route itself is a full-scale passthrough, so the destination total
    // *is* the VCA's value. `uni-b` reads 0.25 and `bi-b` reads −0.5, which
    // folds to the same 0.25.
    Case {
        name: "unipolar scale source passes through, unbent",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), UNI_B, BEND_LIN, ON)],
        sources: &[(BI_A, 1.0), (UNI_B, 0.25)],
        expect: &[(DEST_A, 0.25)],
    },
    Case {
        name: "unipolar scale source, exp bend",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), UNI_B, BEND_EXP, ON)],
        sources: &[(BI_A, 1.0), (UNI_B, 0.25)],
        expect: &[(DEST_A, 0.0625)],
    },
    Case {
        name: "unipolar scale source, log bend",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), UNI_B, BEND_LOG, ON)],
        sources: &[(BI_A, 1.0), (UNI_B, 0.25)],
        expect: &[(DEST_A, 0.5)],
    },
    Case {
        name: "bipolar scale source folds before it bends, unbent",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), BI_B, BEND_LIN, ON)],
        sources: &[(BI_A, 1.0), (BI_B, -0.5)],
        expect: &[(DEST_A, 0.25)],
    },
    Case {
        name: "bipolar scale source, exp bend",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), BI_B, BEND_EXP, ON)],
        sources: &[(BI_A, 1.0), (BI_B, -0.5)],
        expect: &[(DEST_A, 0.0625)],
    },
    Case {
        name: "bipolar scale source, log bend",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), BI_B, BEND_LOG, ON)],
        sources: &[(BI_A, 1.0), (BI_B, -0.5)],
        expect: &[(DEST_A, 0.5)],
    },
    // The gate's two ends, and the clamp that guarantees them. A bipolar scale
    // source at its floor shuts the route completely; an over-range unipolar
    // one cannot push it past its configured depth.
    Case {
        name: "a scale source below the gate shuts the route",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), BI_B, BEND_LIN, ON)],
        sources: &[(BI_A, 1.0), (BI_B, -3.0)],
        expect: &[],
    },
    Case {
        name: "a scale source above the gate cannot exceed full depth",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), UNI_B, BEND_LIN, ON)],
        sources: &[(BI_A, 1.0), (UNI_B, 1.75)],
        expect: &[(DEST_A, 1.0)],
    },
    // ── the scale VCA's own polarity axis (0341) ────────────────────────────
    //
    // Same shape as the block above — a full-scale passthrough route, so the
    // destination total *is* the VCA's value — but now exercising the two
    // settings that do **not** consult the scale source's own polarity.
    //
    // `Abs` is the setting the axis was added for. Under the old fold, a
    // bipolar `voice-spread` scaling a route could only ever mean "the voices
    // on one side of the spread"; these two rows are the same magnitude either
    // side of centre reaching the same gain, which is "the voices at both
    // edges".
    Case {
        name: "abs scale opens at a bipolar source's negative extreme",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), BI_B, SCALE_ABS, ON)],
        sources: &[(BI_A, 1.0), (BI_B, -0.75)],
        expect: &[(DEST_A, 0.75)],
    },
    Case {
        name: "abs scale opens identically at the positive extreme",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), BI_B, SCALE_ABS, ON)],
        sources: &[(BI_A, 1.0), (BI_B, 0.75)],
        expect: &[(DEST_A, 0.75)],
    },
    Case {
        name: "abs scale shuts the route at a bipolar source's centre",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), BI_B, SCALE_ABS, ON)],
        sources: &[(BI_A, 1.0), (BI_B, 0.0)],
        expect: &[],
    },
    Case {
        // Identity on a unipolar source, exactly as `Abs` is on the primary
        // axis: there is nothing to rectify, so this reads the same as the
        // `direct` row two blocks up.
        name: "abs scale on a unipolar source is the identity",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), UNI_B, SCALE_ABS, ON)],
        sources: &[(BI_A, 1.0), (UNI_B, 0.25)],
        expect: &[(DEST_A, 0.25)],
    },
    Case {
        name: "abs scale, exp bend",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), BI_B, SCALE_ABS_EXP, ON)],
        sources: &[(BI_A, 1.0), (BI_B, -0.5)],
        expect: &[(DEST_A, 0.25)],
    },
    Case {
        name: "abs scale, log bend",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), BI_B, SCALE_ABS_LOG, ON)],
        sources: &[(BI_A, 1.0), (BI_B, -0.25)],
        expect: &[(DEST_A, 0.5)],
    },
    // `Bipolar` on the VCA is a threshold-ish gate: shut over the source's
    // lower half, opening across its upper. On a *unipolar* source this is the
    // interesting one, and the row below is also what rules out the tempting
    // "apply the polarity, then fold" order — that would round-trip `2v − 1`
    // straight back to `v` and make this setting a no-op reading 0.625.
    Case {
        name: "bipolar scale gates a unipolar source's upper half",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), UNI_B, SCALE_BIPOLAR, ON)],
        sources: &[(BI_A, 1.0), (UNI_B, 0.625)],
        expect: &[(DEST_A, 0.25)],
    },
    Case {
        name: "bipolar scale shuts below the halfway point",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), UNI_B, SCALE_BIPOLAR, ON)],
        sources: &[(BI_A, 1.0), (UNI_B, 0.25)],
        expect: &[],
    },
    Case {
        // On an already-bipolar source the clamp does most of the work: only
        // `v ≥ 0.5` opens the gate at all. Blunt, and deliberately kept —
        // designing the combination away would mean the two axes are not
        // independent after all.
        name: "bipolar scale on a bipolar source clamps hard",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), BI_B, SCALE_BIPOLAR, ON)],
        sources: &[(BI_A, 1.0), (BI_B, 0.75)],
        expect: &[(DEST_A, 0.5)],
    },
    Case {
        name: "bipolar scale, exp bend",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), UNI_B, SCALE_BIPOLAR_EXP, ON)],
        sources: &[(BI_A, 1.0), (UNI_B, 0.75)],
        expect: &[(DEST_A, 0.25)],
    },
    Case {
        name: "bipolar scale, log bend",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), UNI_B, SCALE_BIPOLAR_LOG, ON)],
        sources: &[(BI_A, 1.0), (UNI_B, 0.625)],
        expect: &[(DEST_A, 0.5)],
    },
    Case {
        name: "an unwired scale source is exact unity",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, 1.0), (UNI_B, 0.25)],
        expect: &[(DEST_A, 1.0)],
    },
    Case {
        // Every evaluator here and in both synths hoists the VCA buffer above
        // the route loop and *resets* it for an unscaled route rather than
        // reallocating. Drop that reset and the second route silently inherits
        // the first's gate — which no single-route case can see.
        name: "an unscaled route after a scaled one is not gated by it",
        routes: &[
            route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), UNI_B, BEND_LIN, ON),
            route(BI_A, DEST_B, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
        ],
        sources: &[(BI_A, 1.0), (UNI_B, 0.25)],
        expect: &[(DEST_A, 0.25), (DEST_B, 1.0)],
    },
    Case {
        // Depth and VCA both away from unity, so a route's gain is the product
        // of three distinct factors rather than one of them wearing the others'
        // identity.
        name: "depth and the VCA both scale the same route",
        routes: &[route(BI_A, DEST_A, 0.5, curve_code(Direct, Lin), UNI_B, BEND_LIN, ON)],
        sources: &[(BI_A, -1.0), (UNI_B, 0.25)],
        expect: &[(DEST_A, -0.125)],
    },
    // ── the on/off switch ───────────────────────────────────────────────────
    Case {
        name: "a switched-off route contributes nothing but keeps its wiring",
        routes: &[
            route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
            route(BI_A, DEST_B, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, OFF),
        ],
        sources: &[(BI_A, 0.5)],
        expect: &[(DEST_A, 0.5)],
    },
    Case {
        // The compaction case for the switch specifically: the off route sits
        // between two live ones sharing a destination, so a path that dropped
        // it at the wrong point would change the sum's order, not just its
        // value.
        name: "a switched-off route is dropped identically between two live ones",
        routes: &[
            route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
            route(UNI_A, DEST_A, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, OFF),
            route(BI_B, DEST_A, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
        ],
        sources: &[(BI_A, 0.5), (UNI_A, 1.0), (BI_B, 0.25)],
        expect: &[(DEST_A, 0.75)],
    },
    // ── several slots summing into one dest ─────────────────────────────────
    Case {
        name: "three routes into one dest sum additively, in slot order",
        routes: &[
            route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
            route(UNI_A, DEST_A, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
            route(BI_B, DEST_A, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
        ],
        sources: &[(BI_A, 0.5), (UNI_A, 0.25), (BI_B, -0.125)],
        expect: &[(DEST_A, 0.625)],
    },
    Case {
        name: "depth scales each contribution before the sum, sign included",
        routes: &[
            route(BI_A, DEST_C, 0.5, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
            route(BI_A, DEST_C, -0.25, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
        ],
        sources: &[(BI_A, 0.5)],
        expect: &[(DEST_C, 0.125)],
    },
    // ── inert slots interleaved with live ones (compaction) ─────────────────
    Case {
        // A full eight-slot table: every way of being inert, interleaved with
        // three live routes to three different destinations.
        name: "inert slots interleaved with live ones compact away",
        routes: &[
            route(NONE, DEST_A, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
            route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
            route(BI_A, DEST_A, 0.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
            route(UNI_A, DEST_B, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
            route(BI_A, NONE, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
            route(BI_A, DEST_C, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, OFF),
            route(BI_B, DEST_D, -1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
            route(NONE, NONE, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON),
        ],
        sources: &[(BI_A, 0.5), (UNI_A, 0.25), (BI_B, 0.5)],
        expect: &[(DEST_A, 0.5), (DEST_B, 0.25), (DEST_D, -0.5)],
    },
    // ── zero-depth and unwired-endpoint short circuits ──────────────────────
    Case {
        name: "an empty table writes a zero accumulator",
        routes: &[],
        sources: &[(BI_A, 1.0), (UNI_A, 1.0)],
        expect: &[],
    },
    Case {
        name: "zero depth short-circuits however loud the source",
        routes: &[route(BI_A, DEST_A, 0.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, 1.0)],
        expect: &[],
    },
    Case {
        name: "an unwired source is inert even with a stale depth",
        routes: &[route(NONE, DEST_A, 99.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, 1.0)],
        expect: &[],
    },
    Case {
        name: "an unwired dest is inert even with a stale depth",
        routes: &[route(BI_A, NONE, 99.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON)],
        sources: &[(BI_A, 1.0)],
        expect: &[],
    },
    // ── out-of-range codes degrade rather than alias ────────────────────────
    //
    // What a corrupt or forward-dated preset blob does. Degrading to
    // `(Direct, Lin)` makes the route audibly plain; aliasing onto a real curve
    // would make it audibly wrong.
    Case {
        name: "a curve code one past the table degrades to direct/lin",
        routes: &[route(BI_A, DEST_A, 1.0, 9, NONE, BEND_LIN, ON)],
        sources: &[(BI_A, -0.25)],
        expect: &[(DEST_A, -0.25)],
    },
    Case {
        name: "a wildly out-of-range curve code degrades too",
        routes: &[route(BI_A, DEST_A, 1.0, 255, NONE, BEND_LIN, ON)],
        sources: &[(BI_A, -0.25)],
        expect: &[(DEST_A, -0.25)],
    },
    Case {
        name: "an out-of-range scale curve degrades to direct/lin",
        routes: &[route(BI_A, DEST_A, 1.0, curve_code(Direct, Lin), UNI_B, 200, ON)],
        sources: &[(BI_A, 1.0), (UNI_B, 0.25)],
        expect: &[(DEST_A, 0.25)],
    },
    Case {
        // The other half of the same encoding: legacy code 3 predates the
        // polarity/shape split and must still mean bipolar-with-a-linear-bend,
        // at the evaluator and not just in the codec's own round-trip test.
        name: "legacy curve code 3 still means bipolar/lin",
        routes: &[route(UNI_A, DEST_A, 1.0, 3, NONE, BEND_LIN, ON)],
        sources: &[(UNI_A, 0.625)],
        expect: &[(DEST_A, 0.25)],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::{N_CURVES, N_POLARITIES, N_SHAPES};
    use crate::test_roster::TestRoster;
    use std::collections::HashSet;

    /// `TestRoster`'s widths, spelled once. The storage guard turns a
    /// disagreement between these and the roster into a compile error.
    const NS: usize = 4;
    const ND: usize = 4;

    /// The whole table, at three lane counts.
    ///
    /// `L = 1` is the lane loop's degenerate case, `L = 4` the width a NEON
    /// vector holds exactly, `L = 8` what both synths actually run. A path that
    /// assumed a full vector, or that handled a tail wrong, disagrees at one of
    /// the three and not the others.
    #[test]
    fn every_case_holds_on_every_path_at_every_lane_count() {
        for case in CASES {
            run_case::<TestRoster, NS, ND, 1>(case);
            run_case::<TestRoster, NS, ND, 4>(case);
            run_case::<TestRoster, NS, ND, 8>(case);
        }
    }

    /// Exact dyadic per-lane weights: distinct, both signs, and a zero, so no
    /// lane can accidentally read another's value and every product stays
    /// exactly representable.
    const LANE_WEIGHTS: [f32; 8] = [1.0, 0.5, -1.0, 0.25, -0.5, 0.0, 0.75, -0.25];

    /// Lanes are independent, and the paths still agree when they are.
    ///
    /// [`CASES`] broadcasts each source across every lane, which is what makes
    /// its expectations readable — but it also means a path that read lane 0
    /// and splatted it would pass the whole table. So run every case again with
    /// each lane on its own inputs, and require both that the paths still agree
    /// bit-exactly and that lane `l` of the bank equals a one-lane run on lane
    /// `l`'s values.
    #[test]
    fn each_lane_evaluates_on_its_own_inputs() {
        const L: usize = LANE_WEIGHTS.len();
        for case in CASES {
            let mut src: SourceLanes<NS, L> = [[0.0; L]; NS];
            for &(si, v) in case.sources {
                for (l, w) in LANE_WEIGHTS.iter().enumerate() {
                    src[si as usize][l] = v * w;
                }
            }

            let paths = eval_paths::<TestRoster, NS, ND, L>();
            assert!(paths.len() >= MIN_EVAL_PATHS);
            let results: Vec<DestLanes<ND, L>> = paths
                .iter()
                .map(|p| {
                    let mut out: DestLanes<ND, L> = [[f32::NAN; L]; ND];
                    (p.eval)(case.routes, &src, &mut out);
                    out
                })
                .collect();
            for (p, got) in paths.iter().zip(&results).skip(1) {
                assert_eq!(
                    bits(got),
                    bits(&results[0]),
                    "'{}': path '{}' disagrees per-lane",
                    case.name,
                    p.name
                );
            }

            for l in 0..L {
                let mut one: SourceLanes<NS, 1> = [[0.0; 1]; NS];
                for (s, row) in one.iter_mut().enumerate() {
                    row[0] = src[s][l];
                }
                let single = eval_paths::<TestRoster, NS, ND, 1>();
                let mut got: DestLanes<ND, 1> = [[f32::NAN; 1]; ND];
                (single[0].eval)(case.routes, &one, &mut got);
                for d in 0..ND {
                    assert_eq!(
                        results[0][d][l].to_bits(),
                        got[d][0].to_bits(),
                        "'{}': lane {l} of dest {d} is not the one-lane result",
                        case.name
                    );
                }
            }
        }
    }

    fn bits<const ND: usize, const L: usize>(d: &DestLanes<ND, L>) -> Vec<u32> {
        d.iter().flatten().map(|v| v.to_bits()).collect()
    }

    /// The table's values are exactly representable on purpose, which is what
    /// makes its expectations checkable by hand — and also what stops it seeing
    /// a **reassociation**. Every grouping of exact dyadics agrees.
    ///
    /// So sweep values that do not round exactly. This is the case the table
    /// covers by construction and the sweep covers by accident: the same
    /// spread vxn-1b's own scalar-vs-bank parity test uses, generalised to
    /// every path this crate offers. A path that folded `gain · vca` at a
    /// different point, or summed a shared destination in a different order,
    /// fails here and nowhere else.
    #[test]
    fn paths_agree_on_values_that_do_not_round_exactly() {
        const L: usize = 8;
        let mut rng = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let paths = eval_paths::<TestRoster, NS, ND, L>();
        assert!(paths.len() >= MIN_EVAL_PATHS);
        for _ in 0..500 {
            // A full table of routes, most of them live, several sharing a
            // destination, with inert ones interleaved — the shapes the table
            // spells out one at a time, drawn together.
            let routes: Vec<Route> = (0..TestRoster::N_SLOTS)
                .map(|_| {
                    let pick = next();
                    let endpoint = |v: u64, n: u64| {
                        // One value in five is the unwired sentinel.
                        let r = v % (n + 1);
                        if r == n { NONE } else { r as u8 }
                    };
                    route(
                        endpoint(pick, TestRoster::N_SOURCES as u64),
                        endpoint(pick >> 8, TestRoster::N_DESTS as u64),
                        // Thirds and sevenths: none of these is exact in f32.
                        ((pick >> 16) % 2001) as f32 / 3000.0 - 0.333,
                        ((pick >> 32) % (N_CURVES as u64 + 2)) as u8,
                        endpoint(pick >> 40, TestRoster::N_SOURCES as u64),
                        ((pick >> 48) % (N_SHAPES as u64 + 1)) as u8,
                        ((pick >> 56) % 3) != 0,
                    )
                })
                .collect();
            let mut src: SourceLanes<NS, L> = [[0.0; L]; NS];
            for row in src.iter_mut() {
                for v in row.iter_mut() {
                    *v = ((next() % 4001) as f32 / 2100.0) - 0.953;
                }
            }

            let results: Vec<DestLanes<ND, L>> = paths
                .iter()
                .map(|p| {
                    let mut out: DestLanes<ND, L> = [[f32::NAN; L]; ND];
                    (p.eval)(&routes, &src, &mut out);
                    out
                })
                .collect();
            for (p, got) in paths.iter().zip(&results).skip(1) {
                assert_eq!(
                    bits(got),
                    bits(&results[0]),
                    "path '{}' is not bit-exact against '{}'",
                    p.name,
                    paths[0].name
                );
            }
        }
    }

    /// The criterion the runner exists to keep honest: with one path there is
    /// nothing to compare, so the runner must say so rather than pass. Pinning
    /// the count here means a ticket that removes a path fails *this* test with
    /// an explanation, not thirty identical assertion failures.
    #[test]
    fn the_runner_has_more_than_one_path_to_compare() {
        let paths = eval_paths::<TestRoster, NS, ND, 8>();
        assert!(
            paths.len() >= MIN_EVAL_PATHS,
            "only {} evaluator path(s); cross-path agreement would be vacuous",
            paths.len()
        );
        let names: HashSet<&str> = paths.iter().map(|p| p.name).collect();
        assert_eq!(names.len(), paths.len(), "two paths share a name");
        // `MIN_EVAL_PATHS` alone would still be satisfied by the harness's own
        // pair, and then the whole table would prove only that this module is
        // consistent with itself — the exact failure the count guard exists to
        // prevent, one level up. Name the shipped paths so dropping one is a
        // failure here rather than a quiet loss of every case's coverage.
        for shipped in ["shared/scalar", "shared/banked"] {
            assert!(
                names.contains(shipped),
                "'{shipped}' is not registered: the case table would no longer \
                 cover the evaluator both synths run"
            );
        }
    }

    /// Case names are what every failure message leads with, so a duplicate
    /// would point at the wrong row.
    #[test]
    fn case_names_are_unique() {
        let names: HashSet<&str> = CASES.iter().map(|c| c.name).collect();
        assert_eq!(names.len(), CASES.len(), "two cases share a name");
    }

    /// The coverage list is the ticket's spec, so hold the table to it rather
    /// than trusting that nobody deletes a row. All nine pairs, reached through
    /// the flat code the rows actually spell — a degradation row therefore
    /// counts toward `direct/lin`, which is exactly what it evaluates as.
    #[test]
    fn polarity_shape_pairs_are_all_covered() {
        let mut seen = [[false; N_SHAPES]; N_POLARITIES];
        for case in CASES {
            for r in case.routes {
                if r.is_live() {
                    let (p, s) = r.axes();
                    seen[p as usize][s as usize] = true;
                }
            }
        }
        for p in Polarity::ALL {
            for s in Shape::ALL {
                assert!(seen[p as usize][s as usize], "no case covers {p:?}/{s:?}");
            }
        }
    }

    /// All four resolved scale folds against all three bends — twelve
    /// combinations, each of which is a separate arm in the hoisted path.
    ///
    /// Four rather than six: `Abs` and `Bipolar` are absolute range maps that
    /// do not consult the scale source's own polarity, so a case reaches
    /// `Rectify` whichever way its scale source swings. The rows still cover
    /// both source polarities under each — see
    /// [`abs_scale_ignores_the_sources_own_polarity`] — because "does not
    /// consult" is itself the claim.
    #[test]
    fn scale_folds_and_bends_are_all_covered() {
        let mut seen = [[false; N_SHAPES]; 4];
        for case in CASES {
            for r in case.routes {
                if r.is_live() && r.scale_src != NONE {
                    let (polarity, bend) = r.scale_axes();
                    let fold =
                        ScaleFold::resolve(polarity, TestRoster::source_is_bipolar(r.scale_src));
                    seen[fold as usize][bend as usize] = true;
                }
            }
        }
        for fold in [
            ScaleFold::Passthrough,
            ScaleFold::Fold,
            ScaleFold::Rectify,
            ScaleFold::AcCouple,
        ] {
            for bend in Shape::ALL {
                assert!(
                    seen[fold as usize][bend as usize],
                    "no case scales a route by a {fold:?} range map with a {bend:?} bend"
                );
            }
        }
    }

    /// Both scale-source polarities are exercised under **each** scale
    /// polarity. The fold table above collapses `Abs`/`Bipolar` across the two,
    /// which is exactly why the case rows must not.
    #[test]
    fn abs_scale_ignores_the_sources_own_polarity() {
        let mut seen = [[false; 2]; N_POLARITIES];
        for case in CASES {
            for r in case.routes {
                if r.is_live() && r.scale_src != NONE {
                    let bipolar = TestRoster::source_is_bipolar(r.scale_src);
                    seen[r.scale_axes().0 as usize][bipolar as usize] = true;
                }
            }
        }
        for polarity in Polarity::ALL {
            for bipolar in [false, true] {
                assert!(
                    seen[polarity as usize][bipolar as usize],
                    "no case scales by a {} source with {polarity:?} scale polarity",
                    if bipolar { "bipolar" } else { "unipolar" }
                );
            }
        }
    }

    /// The rest of the coverage list, which is about a route's *state* rather
    /// than its shaping: the off switch, an unscaled route, each unwired
    /// endpoint, zero depth, an empty table, several routes sharing a dest, and
    /// a code past the tables.
    #[test]
    fn the_short_circuit_and_degradation_cases_are_all_present() {
        let routes = || CASES.iter().flat_map(|c| c.routes);
        assert!(routes().any(|r| !r.enabled), "no switched-off route");
        assert!(routes().any(|r| r.scale_src == NONE), "no unscaled route");
        assert!(routes().any(|r| r.source == NONE), "no unwired source");
        assert!(routes().any(|r| r.dest == NONE), "no unwired dest");
        assert!(routes().any(|r| r.depth == 0.0), "no zero-depth route");
        assert!(
            routes().any(|r| r.curve as usize >= N_CURVES),
            "no out-of-range curve code"
        );
        assert!(
            routes().any(|r| r.scale_curve as usize >= N_CURVES),
            "no out-of-range scale curve"
        );
        assert!(CASES.iter().any(|c| c.routes.is_empty()), "no empty table");
        assert!(
            CASES.iter().any(|c| {
                let live: Vec<u8> =
                    c.routes.iter().filter(|r| r.is_live()).map(|r| r.dest).collect();
                live.len() >= 3 && live.iter().all(|d| *d == live[0])
            }),
            "no case with three live routes into one dest"
        );
        assert!(
            CASES
                .iter()
                .any(|c| c.routes.len() == TestRoster::N_SLOTS
                    && c.routes.iter().any(|r| !r.is_live())
                    && c.routes.iter().any(Route::is_live)),
            "no full-table case interleaving inert and live routes"
        );
    }

    /// A malformed row must name itself, not fall through to the roster's own
    /// panic about an index it was handed.
    #[test]
    #[should_panic(expected = "past the roster")]
    fn a_case_naming_a_dest_the_roster_lacks_fails_on_the_case() {
        let bad = Case {
            name: "bad",
            routes: &[],
            sources: &[],
            expect: &[(9, 1.0)],
        };
        run_case::<TestRoster, NS, ND, 4>(&bad);
    }

    /// And a destination the case does not mention must come out exactly zero,
    /// which is what makes "this route wrote only its own dest" a property of
    /// every row.
    #[test]
    #[should_panic(expected = "dest 1 lane 0 is 0.5")]
    fn an_unmentioned_dest_must_be_zero() {
        const BAD: Case = Case {
            name: "bad",
            routes: &[route(BI_A, DEST_B, 1.0, curve_code(Direct, Lin), NONE, BEND_LIN, ON)],
            sources: &[(BI_A, 0.5)],
            expect: &[],
        };
        run_case::<TestRoster, NS, ND, 4>(&BAD);
    }
}
