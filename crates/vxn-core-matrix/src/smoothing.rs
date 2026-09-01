//! **Target smoothing**: post-sum, per-destination, driven by the declared
//! class ([ADR 0003](../../../../adrs/0003-vxn-core-matrix.md) §3, ticket 0335).
//!
//! The matrix resolves a destination's total once per control block and the
//! render holds it for the block. A stepped source — a square LFO, a note-random
//! latch, a fast envelope — routed into a continuous destination therefore lands
//! a hard value step at every block edge (~1.5 kHz at 48 kHz), and that step is
//! a click. Smoothing is what turns the stair into a slope.
//!
//! Two decisions this module encodes, both of them settled rather than assumed:
//!
//! - **Post-sum, not per-route.** The filters are linear, so filtering each
//!   route and then summing is arithmetically identical to summing and then
//!   filtering — at N× the cost and N× the state for N slots sharing a
//!   destination.
//! - **Per-destination, not uniform.** The right time constant is a property of
//!   how click-prone the target is, not of the source driving it. `delay-mix`
//!   never clicks; pitch stairsteps audibly at every block edge. So the class is
//!   the `smooth =` column of a destination's roster row
//!   ([`Smoothing`](crate::roster::Smoothing)), and
//!   [`class_rows`] is how a synth turns that column into the bank's row set.
//!
//! ## The cascade is load-bearing
//!
//! [`CascadeBank`] is **two** one-poles and must not be "simplified" to one. A
//! single pole is C0 but C1-broken: at a saw or pulse LFO step the output
//! *value* is continuous while its *velocity* jumps 0 → max, and that velocity
//! step is the click. A second pole makes the output slope start at zero, so
//! sharp shapes routed to pitch ramp in clean. Both synths arrived at two poles
//! independently, for the same destination, before either shared any code.
//!
//! ## Two tick shapes, because the synths schedule differently
//!
//! Every bank offers a **bank-wide** tick ([`CascadeBank::tick_rows`]) and a
//! **per-lane** one ([`CascadeBank::tick_lane`]). That is not indecision: the
//! recurrence, the state, the snap and the settle predicate are shared — those
//! are the parts that were written twice — while *when* to advance a lane is a
//! render-loop property the ticket deliberately leaves per-synth.
//!
//! vxn-2 ticks its whole stack every `PITCH_SMOOTH_QUANTUM` samples and reads
//! the result back through a projection. vxn-1b ticks **only lanes with a live
//! route**, gated by masks that also decide whether to re-cook that lane's
//! oscillator increment, pulse width, PM index or pan gains — the tick and the
//! cook are one branch. Flattening that into a bank-wide tick would advance
//! lanes that currently freeze, and on a *pitch* destination an ULP-scale
//! difference integrates into phase drift (E049 §"The bar"). The gating stays.
//!
//! ## The fused tick loop is deliberate — the two-pass split was tried
//!
//! [`CascadeBank::tick_rows`] advances both stages in **one** loop body, stage 2
//! reading the stage-1 element just written. 0335 proposed splitting that into
//! two flat passes on the premise that the loop-carried dependency forces the
//! vectoriser to interleave with `zip2`/`uzp2` shuffles, and priced the fix at
//! 46%. **That premise does not hold on this toolchain**, and the split is a
//! regression. Post-LTO `llvm-objdump` on a linked bench binary, both shapes at
//! `NR = 8`, `L = 8`: 138 instructions and 96 `.4s` ops each, and **zero**
//! `zip`/`uzp`/`trn` in either — the fused form already vectorises cleanly. It
//! is also faster, 7.01 ns against 7.52 ns (+7.3%), three interleaved standalone
//! rounds. Splitting only inserts a serialisation point between the stages
//! without removing any work.
//!
//! Do not re-split it without re-measuring. And measure standalone: the same
//! comparison inside one binary reported the split *ahead*, which was code
//! layout rather than the loop.
//!
//! ## What stays synth-side
//!
//! The **absences**, and one exception. A destination declared
//! [`Smoothing::Block`] gets no filter — that is a decision, not an omission,
//! and adding smoothing to a destination that lacks it today is a behaviour
//! change with a listening test behind it, not a consequence of this module
//! existing. And vxn-1b smooths only the *non-envelope* part of its VCA
//! coefficient: the envelope part is per-frame exact and smoothing it would
//! smear the attack. That factoring is a property of vxn-1b's VCA rather than of
//! routing, so `Amp` declares `Block` and the synth runs its own
//! [`OnePoleBank`] over the part it chooses. ADR 0003 §3 names it as the one
//! acknowledged exception; it is a deliberate limit on the abstraction, not a
//! gap to close.

use crate::roster::Smoothing;

// ── deriving a bank's rows from the declared column ─────────────────────────

/// How many destinations declare `class`, given the roster's smoothing column
/// indexed by **storage** row.
///
/// A `const fn` over a slice rather than a method on
/// [`MatrixRoster`](crate::roster::MatrixRoster), because a bank's row count has
/// to be an array length: stable Rust will take this in a `const` initialiser
/// and will not take a trait method call. A synth spells its column once as a
/// `const` built from its own `DestId::ALL` and feeds it to both this and
/// [`class_rows`].
///
/// ```
/// # use vxn_core_matrix::roster::Smoothing;
/// # use vxn_core_matrix::smoothing::{class_count, class_rows};
/// const COL: [Smoothing; 4] =
///     [Smoothing::Block, Smoothing::QuantumCascade, Smoothing::Quantum, Smoothing::QuantumCascade];
/// const N: usize = class_count(&COL, Smoothing::QuantumCascade);
/// const ROWS: [usize; N] = class_rows(&COL, Smoothing::QuantumCascade);
/// assert_eq!(N, 2);
/// assert_eq!(ROWS, [1, 3]);
/// ```
pub const fn class_count(column: &[Smoothing], class: Smoothing) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < column.len() {
        // `as u8` rather than `==`: `PartialEq` is not const-callable on stable.
        if column[i] as u8 == class as u8 {
            n += 1;
        }
        i += 1;
    }
    n
}

/// The storage rows of every destination declaring `class`, **in roster order**.
///
/// Row `i` of a bank sized `NR = class_count(column, class)` smooths
/// destination `class_rows(column, class)[i]`. Order is the roster's, so a bank
/// row moves the day a new destination of that class is declared ahead of an
/// existing one — which is why a consumer asks for its row by name
/// ([`row_of`]) rather than writing a literal down.
///
/// Panics at compile time if `NR` disagrees with the count, which is what makes
/// the pair impossible to get out of step.
pub const fn class_rows<const NR: usize>(column: &[Smoothing], class: Smoothing) -> [usize; NR] {
    assert!(
        NR == class_count(column, class),
        "bank width must be exactly the number of destinations declaring this class"
    );
    let mut rows = [0usize; NR];
    let mut n = 0;
    let mut i = 0;
    while i < column.len() {
        if column[i] as u8 == class as u8 {
            rows[n] = i;
            n += 1;
        }
        i += 1;
    }
    rows
}

/// Which bank row carries destination storage row `dest`, or `None` for a
/// destination this class does not smooth.
///
/// `pub` and total rather than panicking: a `const` call site wants the compile
/// error, but a runtime caller walking classes it did not choose wants a value
/// to branch on rather than an audio-thread panic.
pub const fn row_of<const NR: usize>(rows: &[usize; NR], dest: usize) -> Option<usize> {
    let mut i = 0;
    while i < NR {
        if rows[i] == dest {
            return Some(i);
        }
        i += 1;
    }
    None
}

// ── the banks ───────────────────────────────────────────────────────────────

/// A single one-pole per (row, lane): `state += coeff · (target − state)`.
///
/// The coefficient belongs to the **bank**, not to a row — it is a property of
/// the class and the rate the synth ticks at, and both synths already held it
/// that way. Keeping it off the row is what lets a new smoothed destination cost
/// one row rather than a row plus a coefficient plus four methods.
#[derive(Clone, Copy, Debug)]
pub struct OnePoleBank<const NR: usize, const L: usize> {
    state: [[f32; L]; NR],
    coeff: f32,
}

/// Two cascaded one-poles per (row, lane). See the module docs on why the
/// second pole is not optional.
#[derive(Clone, Copy, Debug)]
pub struct CascadeBank<const NR: usize, const L: usize> {
    /// First stage (intermediate). **Not** the output — see [`Self::current`].
    stage1: [[f32; L]; NR],
    /// Second stage, and the smoothed output.
    state: [[f32; L]; NR],
    coeff: f32,
}

impl<const NR: usize, const L: usize> OnePoleBank<NR, L> {
    /// A settled bank with the given per-tick coefficient. The caller cooks it,
    /// because the tick *rate* is its render loop's business — a bank ticked per
    /// quantum and one ticked per frame want different coefficients for the same
    /// time constant.
    pub fn new(coeff: f32) -> Self {
        Self { state: [[0.0; L]; NR], coeff }
    }

    /// Zero every row and lane (engine reset). Coefficients survive: they are
    /// already cooked for the current sample rate.
    pub fn clear(&mut self) {
        self.state = [[0.0; L]; NR];
    }

    /// The smoothed output. Row-major, `[row][lane]`.
    #[inline]
    pub fn current(&self) -> &[[f32; L]; NR] {
        &self.state
    }

    /// One lane's current value, without advancing it.
    #[inline]
    pub fn current_lane(&self, row: usize, lane: usize) -> f32 {
        self.state[row][lane]
    }

    /// Snap one lane straight to `target` — a fresh note starts settled rather
    /// than gliding up from whatever the stolen voice left behind.
    #[inline]
    pub fn snap_lane(&mut self, row: usize, lane: usize, target: f32) {
        self.state[row][lane] = target;
    }

    /// Advance one lane one step toward `target` and return the new value.
    #[inline]
    pub fn tick_lane(&mut self, row: usize, lane: usize, target: f32) -> f32 {
        let s = &mut self.state[row][lane];
        *s += self.coeff * (target - *s);
        *s
    }

    /// Whether one lane is worth ticking. False **only** when the lane is at
    /// rest *at zero* with nothing to chase — which is the case a render loop
    /// wants to skip, because then its block-start value is already right.
    ///
    /// Read the two clauses separately, because the second is the surprising
    /// one: a lane still chasing its target (`|target − state| > eps`), **or** a
    /// lane displaced from zero at all (`|state| > eps`). So a lane parked
    /// exactly on a nonzero target reports active even though a tick will not
    /// move it. That is deliberate and load-bearing: the second clause is what
    /// keeps a lane ticking *after its route turns off*, so it glides back to
    /// zero rather than snapping — and snapping is the click the smoother
    /// exists to prevent. Narrowing it to "still chasing" would introduce one.
    ///
    /// Distinct from [`Self::lane_settled`], which asks only about the distance
    /// to the target. The parked-on-a-nonzero-target lane is settled *and*
    /// active; the two are not complements.
    #[inline]
    pub fn lane_active(&self, row: usize, lane: usize, target: f32, eps: f32) -> bool {
        let s = self.state[row][lane];
        (target - s).abs() > eps || s.abs() > eps
    }

    /// Whether one lane has arrived at `target`.
    #[inline]
    pub fn lane_settled(&self, row: usize, lane: usize, target: f32, eps: f32) -> bool {
        (self.state[row][lane] - target).abs() <= eps
    }
}

impl<const NR: usize, const L: usize> CascadeBank<NR, L> {
    /// A settled bank with the given per-tick coefficient, shared by both
    /// stages. See [`OnePoleBank::new`] on why the caller cooks it.
    pub fn new(coeff: f32) -> Self {
        Self {
            stage1: [[0.0; L]; NR],
            state: [[0.0; L]; NR],
            coeff,
        }
    }

    /// Zero **both** stages (engine reset). Clearing only the output would
    /// strand energy in stage 1 that the next tick would then deliver.
    pub fn clear(&mut self) {
        self.stage1 = [[0.0; L]; NR];
        self.state = [[0.0; L]; NR];
    }

    /// The smoothed output — stage 2. Row-major, `[row][lane]`.
    #[inline]
    pub fn current(&self) -> &[[f32; L]; NR] {
        &self.state
    }

    /// One lane's current output, without advancing it.
    #[inline]
    pub fn current_lane(&self, row: usize, lane: usize) -> f32 {
        self.state[row][lane]
    }

    /// Advance one lane one step toward `target` and return the new output.
    ///
    /// Stage 1 chases the target; stage 2 chases stage 1. The order is the
    /// contract: stage 2 reads the value stage 1 has *just* been given, which is
    /// what [`Self::tick_rows`] reproduces by running its two passes in this
    /// order over the same span.
    #[inline]
    pub fn tick_lane(&mut self, row: usize, lane: usize, target: f32) -> f32 {
        let a = self.coeff;
        self.stage1[row][lane] += a * (target - self.stage1[row][lane]);
        self.state[row][lane] += a * (self.stage1[row][lane] - self.state[row][lane]);
        self.state[row][lane]
    }

    /// Advance every row and lane one step toward the caller's targets, gathered
    /// one row at a time by `pick`.
    ///
    /// `pick(i)` returns bank row `i`'s per-lane target span. A dest-major
    /// accumulator makes that a borrow rather than a copy: row
    /// `rows[i]` of the accumulator *is* this bank row's target.
    ///
    /// **One fused loop, not two passes** — measured, against the ticket's own
    /// expectation. See the module docs: both stages in one body vectorise to
    /// 96 `.4s` ops with no interleaving shuffles, and splitting them costs
    /// 7.3%.
    #[inline]
    pub fn tick_rows<'a>(&mut self, mut pick: impl FnMut(usize) -> &'a [f32; L]) -> &[[f32; L]; NR] {
        let a = self.coeff;
        for (i, (s1, st)) in self.stage1.iter_mut().zip(self.state.iter_mut()).enumerate() {
            let target = pick(i);
            for k in 0..L {
                s1[k] += a * (target[k] - s1[k]);
                st[k] += a * (s1[k] - st[k]);
            }
        }
        &self.state
    }

    /// Snap one lane's **both** stages to `target`, so a re-armed voice starts
    /// settled rather than mid-ramp or gliding from the previous note.
    #[inline]
    pub fn snap_lane(&mut self, row: usize, lane: usize, target: f32) {
        self.stage1[row][lane] = target;
        self.state[row][lane] = target;
    }

    /// Snap every row and lane, targets gathered as in [`Self::tick_rows`].
    #[inline]
    pub fn snap_rows<'a>(&mut self, mut pick: impl FnMut(usize) -> &'a [f32; L]) {
        for i in 0..NR {
            let target = *pick(i);
            self.stage1[i] = target;
            self.state[i] = target;
        }
    }

    /// Whether one lane is worth ticking — the same predicate as
    /// [`OnePoleBank::lane_active`], including the part where a lane parked on a
    /// nonzero target still counts as active. Read that method's note.
    ///
    /// **Both** stages are tested, not just the output: stage 1 can still hold
    /// energy stage 2 has yet to see, and freezing on the output alone strands
    /// the smoother short of its target.
    #[inline]
    pub fn lane_active(&self, row: usize, lane: usize, target: f32, eps: f32) -> bool {
        target.abs() > eps
            || self.state[row][lane].abs() > eps
            || self.stage1[row][lane].abs() > eps
    }

    /// Whether every row and lane sits within `eps` of its target, so a render
    /// loop can skip the tick and whatever recook depends on it.
    ///
    /// Both stages again, for the reason on [`Self::lane_active`]: the output
    /// can pass *through* the target while stage 1 is still mid-ramp.
    #[inline]
    pub fn converged<'a>(&self, mut pick: impl FnMut(usize) -> &'a [f32; L], eps: f32) -> bool {
        for i in 0..NR {
            let target = pick(i);
            for k in 0..L {
                if (self.state[i][k] - target[k]).abs() > eps
                    || (self.stage1[i][k] - target[k]).abs() > eps
                {
                    return false;
                }
            }
        }
        true
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    const COL: [Smoothing; 6] = [
        Smoothing::Block,
        Smoothing::QuantumCascade,
        Smoothing::Quantum,
        Smoothing::QuantumCascade,
        Smoothing::Block,
        Smoothing::PerSample,
    ];

    #[test]
    fn a_class_yields_its_rows_in_roster_order() {
        assert_eq!(class_count(&COL, Smoothing::QuantumCascade), 2);
        assert_eq!(class_count(&COL, Smoothing::Block), 2);
        assert_eq!(class_count(&COL, Smoothing::Quantum), 1);
        assert_eq!(class_count(&COL, Smoothing::PerSample), 1);
        let rows: [usize; 2] = class_rows(&COL, Smoothing::QuantumCascade);
        assert_eq!(rows, [1, 3]);
    }

    /// A class nothing declares is a zero-width bank, not a panic — vxn-1b
    /// declares no `per_sample` row and must still be able to ask.
    #[test]
    fn an_unused_class_is_empty_rather_than_an_error() {
        const NONE_COL: [Smoothing; 2] = [Smoothing::Block, Smoothing::Block];
        assert_eq!(class_count(&NONE_COL, Smoothing::Quantum), 0);
        let rows: [usize; 0] = class_rows(&NONE_COL, Smoothing::Quantum);
        assert!(rows.is_empty());
    }

    /// Row lookup is by name, because [`class_rows`]'s order is the roster's and
    /// moves the day a destination of that class is declared ahead of another.
    #[test]
    fn row_of_names_a_row_and_declines_a_destination_of_another_class() {
        const ROWS: [usize; 2] = class_rows(&COL, Smoothing::QuantumCascade);
        assert_eq!(row_of(&ROWS, 1), Some(0));
        assert_eq!(row_of(&ROWS, 3), Some(1));
        // Row 2 is `Quantum`; this bank does not smooth it.
        assert_eq!(row_of(&ROWS, 2), None);
    }

    fn targets<'a, const NR: usize, const L: usize>(
        t: &'a [[f32; L]; NR],
    ) -> impl FnMut(usize) -> &'a [f32; L] {
        |i| &t[i]
    }

    /// The property the second pole exists for: the output's slope starts at
    /// zero. A single pole's first step is `coeff · target`; the cascade's must
    /// be far smaller, because stage 2 chases a still-near-zero stage 1. That
    /// near-zero starting slope is what kills the click on a stepped source.
    #[test]
    fn the_cascade_output_slope_starts_at_zero() {
        let mut b = CascadeBank::<1, 1>::new(0.4);
        let t = [[1.0f32; 1]; 1];
        let first = b.tick_rows(targets(&t))[0][0];
        assert!(first > 0.0, "it must start moving, got {first}");
        assert!(first < 0.4 * 0.4 + 1e-6, "a lone pole would step 0.4, got {first}");
        assert_eq!(first, 0.4 * 0.4, "stage 2 sees stage 1's *new* value");
    }

    /// Both tick shapes are the same filter. vxn-2 advances a whole bank at
    /// once and vxn-1b one gated lane at a time; if those two ever disagreed,
    /// the same patch would smooth differently depending on which synth ran it.
    #[test]
    fn the_bank_wide_and_per_lane_ticks_agree_bit_exactly() {
        const NR: usize = 3;
        const L: usize = 4;
        let mut wide = CascadeBank::<NR, L>::new(0.3);
        let mut lane = CascadeBank::<NR, L>::new(0.3);
        let mut t = [[0.0f32; L]; NR];
        for (i, row) in t.iter_mut().enumerate() {
            for (k, v) in row.iter_mut().enumerate() {
                *v = 0.37 * (i as f32 + 1.0) - 0.11 * k as f32;
            }
        }
        for _ in 0..64 {
            wide.tick_rows(targets(&t));
            for i in 0..NR {
                for k in 0..L {
                    lane.tick_lane(i, k, t[i][k]);
                }
            }
        }
        for i in 0..NR {
            for k in 0..L {
                assert_eq!(
                    wide.current()[i][k].to_bits(),
                    lane.current()[i][k].to_bits(),
                    "row {i} lane {k}"
                );
            }
        }
    }

    #[test]
    fn a_cascade_converges_on_its_target() {
        let mut b = CascadeBank::<1, 1>::new(0.25);
        let t = [[3.0f32; 1]; 1];
        for _ in 0..256 {
            b.tick_rows(targets(&t));
        }
        assert!((b.current()[0][0] - 3.0).abs() < 1e-3);
    }

    /// A snap lands settled — a tick straight afterwards must not move, or a
    /// fresh note would glide in from the target it was just placed on.
    #[test]
    fn a_snapped_lane_does_not_glide() {
        let mut b = CascadeBank::<2, 2>::new(0.3);
        b.snap_lane(1, 0, 5.0);
        assert_eq!(b.tick_lane(1, 0, 5.0), 5.0);
        // …and the untouched lanes stayed put.
        assert_eq!(b.current_lane(1, 1), 0.0);
        assert_eq!(b.current_lane(0, 0), 0.0);
    }

    /// `snap_rows` must take **both** stages, or the next tick delivers stage
    /// 1's stale energy into an output that was just placed correctly.
    #[test]
    fn snapping_a_whole_bank_takes_both_stages() {
        let mut b = CascadeBank::<2, 2>::new(0.3);
        let moving = [[1.0f32; 2]; 2];
        for _ in 0..4 {
            b.tick_rows(targets(&moving));
        }
        let t = [[-0.5f32; 2]; 2];
        b.snap_rows(targets(&t));
        b.tick_rows(targets(&t));
        assert_eq!(b.current()[0][0], -0.5, "stage 1 still held energy");
    }

    /// `converged` tests both stages: the output can pass *through* the target
    /// while stage 1 is still mid-ramp, and stopping there strands it short.
    #[test]
    fn converged_is_not_satisfied_by_the_output_alone() {
        let mut b = CascadeBank::<1, 1>::new(0.5);
        let up = [[1.0f32; 1]; 1];
        for _ in 0..40 {
            b.tick_rows(targets(&up));
        }
        assert!(b.converged(targets(&up), 1e-4));
        // Drop the target: the output is now far from it, so not converged.
        let zero = [[0.0f32; 1]; 1];
        assert!(!b.converged(targets(&zero), 1e-4));
    }

    /// Clearing must take both stages too, for the same reason.
    #[test]
    fn clear_zeroes_both_stages() {
        let mut b = CascadeBank::<1, 1>::new(0.5);
        let t = [[1.0f32; 1]; 1];
        b.tick_rows(targets(&t));
        b.clear();
        let zero = [[0.0f32; 1]; 1];
        assert!(b.converged(targets(&zero), 0.0), "residual stage-1 energy survived");
    }

    /// `lane_active` and `lane_settled` are different questions, and not
    /// complements. Only rest *at zero* is inactive; a lane parked on a nonzero
    /// target is settled and still active, which is what keeps it ticking after
    /// its route turns off so it glides back down instead of snapping.
    #[test]
    fn active_and_settled_ask_different_questions() {
        let mut b = OnePoleBank::<1, 1>::new(0.5);
        b.snap_lane(0, 0, 0.8);
        // Parked on a nonzero target: arrived, but displaced from zero.
        assert!(b.lane_settled(0, 0, 0.8, 1e-4));
        assert!(b.lane_active(0, 0, 0.8, 1e-4), "displaced from zero is still active");
        // Route turns off: target 0, state 0.8. Neither settled nor inactive —
        // it has to glide down, because snapping is the click.
        assert!(!b.lane_settled(0, 0, 0.0, 1e-4));
        assert!(b.lane_active(0, 0, 0.0, 1e-4));
        // Rest at zero is the only inactive state.
        b.snap_lane(0, 0, 0.0);
        assert!(!b.lane_active(0, 0, 0.0, 1e-4));
        assert!(b.lane_settled(0, 0, 0.0, 1e-4));
    }

    #[test]
    fn a_one_pole_glides_and_lanes_stay_independent() {
        let mut b = OnePoleBank::<2, 2>::new(0.25);
        let first = b.tick_lane(0, 0, 1.0);
        assert_eq!(first, 0.25);
        assert_eq!(b.current_lane(0, 1), 0.0, "the other lane never moved");
        assert_eq!(b.current_lane(1, 0), 0.0, "the other row never moved");
        for _ in 0..256 {
            b.tick_lane(0, 0, 1.0);
        }
        assert!((b.current_lane(0, 0) - 1.0).abs() < 1e-4);
    }
}
