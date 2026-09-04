//! Lane timing **geometry** — beat markers, derived subdivision markers, and the
//! swing warp that positions the latter (ADR 0007 §2/§3, ticket 0347).
//!
//! Two tiers of marker. **Beat markers** `m[0..=n]` are stored and user-draggable;
//! **subdivision markers** are never stored, they are computed on demand:
//!
//! ```text
//! sub_pos(b, k) = m[b] + w(k / n_b) · (m[b+1] - m[b])
//! ```
//!
//! `n_b` is the beat's subdivision count — a lane default with a per-beat override,
//! which is where tuplets live: one beat with `n = 3` inside an otherwise-16ths lane,
//! no separate concept and no special case. `w` is the swing warp: monotonic on
//! `[0, 1]` with `w(0) = 0`, `w(1) = 1`. Beat marker `k = 0` *is* a subdivision
//! marker, so the snap-target set is exactly the subdivision markers.
//!
//! Swing is a **warp on the beat's unit interval** rather than a per-position offset
//! table for one reason (ADR 0007 §3): it generalises over `n_b`. One swing control
//! stays meaningful whatever a beat's sub-count, `n = 3` needs no special case, and
//! derived markers cannot drift out of step with the beat markers that generate them.
//!
//! **[`MIN_SLOT`] is load-bearing, not cosmetic.** Ticket 0348 stores a hit as
//! `t = sub_pos(b, k) + f · (sub_pos(b, k+1) - sub_pos(b, k)) + nudge`; a zero-width
//! slot makes `f` unresolvable and divides by ~0, silently poisoning downstream hits
//! with NaN. Every mutation path here therefore goes through the clamp, and there is
//! no `&mut` into the marker array to bypass it.
//!
//! Pure data and math: no scheduler, no UI, **no allocation on any query path**, and
//! storage is fixed-capacity arrays sized from [`MAX_BEATS`] / [`MAX_SUBS`] so the
//! whole grid is `Copy` and audio-thread safe. Marker *editing* semantics (a drag
//! rubber-bands the hits it owns, insert/delete preserves absolute time) are 0349;
//! this module owns the geometry and its invariants only.
//!
//! This supersedes [`crate::sequencer::Pattern::step_beats`], which stays until 0348
//! removes it. The ADR 0001 §2 polymeter it provides is not lost — marker sets are
//! per-lane, which is what preserves it.

/// Maximum beat slots in one lane's grid (storage ceiling; the live count may be
/// shorter, exactly as [`crate::sequencer::MAX_STEPS`] ceilings a pattern's `len`).
///
/// Sixteen beats is four bars of 4/4. At the lane default of four subdivisions per
/// beat that is 64 snap targets — four times the old 16-step grid — while the marker
/// array stays 17 × `f64` = 136 bytes, so [`Grid`] remains `Copy` and cheap enough to
/// hand across the swap boundary without a heap.
pub const MAX_BEATS: usize = 16;

/// Beat markers *bound* the beat slots, so there is always one more of them than
/// there are beats: `m[0] .. m[n_beats]` inclusive.
pub const MAX_MARKERS: usize = MAX_BEATS + 1;

/// Maximum subdivisions inside a single beat. Sixteen is 64th notes — past the point
/// where a snap target is distinguishable, and it keeps the inverse scan in
/// [`Grid::locate`] bounded by a handful of comparisons.
pub const MAX_SUBS: u32 = 16;

/// Minimum width of a beat slot, in beats — 1/64 of a quarter note (~10 ms at 90 bpm).
///
/// A hard constant here rather than a UI concern: a zero-width slot makes 0348's
/// position fraction unresolvable and its inverse mapping divide by ~0. Exactly
/// representable in binary, so the clamp arithmetic does not itself introduce drift.
pub const MIN_SLOT: f64 = 1.0 / 64.0;

/// Shape of the swing warp `w`. Minimal set (0347): straight plus the classic
/// piecewise-linear MPC pull. Widen behind this enum without a format break — the
/// tag is a `u8` and an unknown tag falls back to the default, matching
/// [`crate::sequencer::RetrigCurve`] and [`crate::flavour::Curve`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum SwingShape {
    /// Identity — `w(u) = u`, evenly spaced subdivisions. Bit-exact, so a straight
    /// lane reproduces the old uniform grid rather than approximating it.
    #[default]
    Straight,
    /// Classic piecewise-linear MPC swing: a single knee at the half-beat that pulls
    /// the odd subdivisions late (or early, for a negative amount).
    Mpc,
}

impl SwingShape {
    #[inline]
    pub fn as_u8(self) -> u8 {
        match self {
            SwingShape::Straight => 0,
            SwingShape::Mpc => 1,
        }
    }

    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => SwingShape::Mpc,
            _ => SwingShape::Straight,
        }
    }
}

/// Ratio the [`SwingShape::Mpc`] knee reaches at `amount = 1`: the half-beat lands
/// 75% of the way through, the classic MPC ceiling. A negative amount mirrors it to
/// 25% (pull early), so `s` never leaves `(0, 1)` and the warp stays strictly
/// increasing across the whole control range.
const MPC_MAX_RATIO: f64 = 0.75;

/// The swing warp: a shape plus its amount. `amount` is a bipolar `-1..1` control —
/// positive pulls the odd subdivisions late (the usual direction), negative early,
/// zero straight. Out-of-range and non-finite values are treated as their clamp, so
/// no caller can produce a non-monotonic `w`.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub struct Swing {
    pub shape: SwingShape,
    pub amount: f64,
}

impl Swing {
    /// No swing — `w` is the identity.
    #[inline]
    pub fn straight() -> Self {
        Self { shape: SwingShape::Straight, amount: 0.0 }
    }

    /// Classic MPC swing at `amount` (`-1..1`, positive = late).
    #[inline]
    pub fn mpc(amount: f64) -> Self {
        Self { shape: SwingShape::Mpc, amount }
    }

    /// The warp `w: [0, 1] → [0, 1]`. **Strictly increasing with fixed endpoints**
    /// `w(0) = 0`, `w(1) = 1` — the two properties [`Grid::sub_pos`] relies on to
    /// keep subdivision markers strictly increasing and inside their beat, whatever
    /// the sub-count.
    ///
    /// The endpoints are returned by an explicit branch rather than falling out of
    /// the arithmetic, so they are exact for every shape that is ever added here.
    #[inline]
    pub fn w(self, u: f64) -> f64 {
        if u <= 0.0 || u.is_nan() {
            return 0.0;
        }
        if u >= 1.0 {
            return 1.0;
        }
        match self.shape {
            SwingShape::Straight => u,
            SwingShape::Mpc => {
                if !self.amount.is_finite() {
                    return u;
                }
                // Knee position: where the half-beat lands. s ∈ [0.25, 0.75], so both
                // limbs have positive slope (2s and 2(1-s)) and w stays strictly
                // increasing. At amount = 0 both limbs collapse to the identity
                // *exactly* — every operation below is a dyadic scale or a Sterbenz
                // subtraction, so a nominally-Mpc lane at zero swing is bit-identical
                // to a straight one.
                let a = self.amount.clamp(-1.0, 1.0);
                let s = 0.5 + a * (MPC_MAX_RATIO - 0.5);
                if u <= 0.5 {
                    (u + u) * s
                } else {
                    s + (u + u - 1.0) * (1.0 - s)
                }
            }
        }
    }
}

/// A beat position resolved against the grid: subdivision `sub` of beat `beat`, plus
/// the fraction `frac` of the way to the next subdivision marker.
///
/// `frac` is a fraction of the **warped** slot (position space), not of the beat's
/// unit interval — it is exactly the `f` of 0348's hit encoding. [`Grid::locate`]
/// returns `frac ∈ [0, 1)` everywhere except the pattern's end marker, which has no
/// following slot and so resolves to the last subdivision with `frac = 1`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GridPos {
    pub beat: usize,
    pub sub: u32,
    pub frac: f64,
}

/// One lane's timing geometry: stored beat markers, per-beat subdivision counts, and
/// the swing warp. Markers are strictly increasing **by construction** — every
/// mutation runs through the [`MIN_SLOT`] clamp, and the array is private.
///
/// Marker positions are absolute beat positions within the pattern, with `m[0]` pinned
/// to `0` and `m[n_beats]` pinned to the pattern length: the outer markers are the
/// pattern bounds, not draggable, because a hit before `m[0]` would have no owning slot.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Grid {
    /// `n_beats + 1` live entries; the tail past the end marker is unused padding,
    /// held equal to the end marker so the derived `PartialEq` compares two grids by
    /// their geometry and not by leftovers from how they were built.
    markers: [f64; MAX_MARKERS],
    /// Live beat count, always `1..=MAX_BEATS`.
    n_beats: usize,
    /// Lane-wide subdivision count, always `1..=MAX_SUBS`.
    default_subs: u32,
    /// Per-beat override; `0` = none, use [`Grid::default_subs`]. This is where
    /// tuplets live (ADR 0007 §2).
    sub_override: [u8; MAX_BEATS],
    swing: Swing,
}

impl Default for Grid {
    /// Four beats of four — the 16-step 16ths grid the step model shipped, straight.
    fn default() -> Self {
        Self::uniform(4, 4.0, 4)
    }
}

impl Grid {
    /// A straight grid: `n_beats` equal beat slots spanning `[0, len_beats]`, `subs`
    /// subdivisions in each, no swing.
    ///
    /// `n_beats` clamps to `1..=MAX_BEATS`, `subs` to `1..=MAX_SUBS`, and `len_beats`
    /// to at least `n_beats · MIN_SLOT` — below that no arrangement of markers can
    /// satisfy the minimum-slot invariant at all.
    pub fn uniform(n_beats: usize, len_beats: f64, subs: u32) -> Self {
        let n = n_beats.clamp(1, MAX_BEATS);
        let len = sane_len(len_beats, n);
        let step = len / n as f64;
        let mut markers = [len; MAX_MARKERS];
        for (i, m) in markers.iter_mut().enumerate().take(n) {
            // `i · step`, not a running sum: no accumulated rounding, and for a dyadic
            // `step` this is bit-identical to the old `i · step_beats` grid.
            *m = i as f64 * step;
        }
        markers[n] = len; // pinned exactly, rather than left as n · step
        Self {
            markers,
            n_beats: n,
            default_subs: subs.clamp(1, MAX_SUBS),
            sub_override: [0; MAX_BEATS],
            swing: Swing::straight(),
        }
    }

    // ── queries ───────────────────────────────────────────────────────────────

    /// Live beat count (`1..=MAX_BEATS`).
    #[inline]
    pub fn n_beats(&self) -> usize {
        self.n_beats
    }

    /// Pattern length in beats — the pinned end marker.
    #[inline]
    pub fn len_beats(&self) -> f64 {
        self.markers[self.n_beats]
    }

    /// Beat marker `i` in beats. `i` clamps into `0..=n_beats`, so the outer markers
    /// answer for any out-of-range index rather than panicking on the audio thread.
    #[inline]
    pub fn beat_marker(&self, i: usize) -> f64 {
        self.markers[i.min(self.n_beats)]
    }

    /// Subdivision count for `beat`: the per-beat override if set, else the lane
    /// default. Always `>= 1`.
    #[inline]
    pub fn subs(&self, beat: usize) -> u32 {
        match self.sub_override.get(beat).copied().unwrap_or(0) {
            0 => self.default_subs,
            n => (n as u32).clamp(1, MAX_SUBS),
        }
    }

    /// The lane-wide subdivision count.
    #[inline]
    pub fn default_subs(&self) -> u32 {
        self.default_subs
    }

    /// The per-beat override, if any (`None` = the beat follows the lane default).
    #[inline]
    pub fn sub_override(&self, beat: usize) -> Option<u32> {
        match self.sub_override.get(beat).copied().unwrap_or(0) {
            0 => None,
            n => Some((n as u32).clamp(1, MAX_SUBS)),
        }
    }

    /// Total subdivision markers across the pattern — the size of the snap-target set.
    pub fn total_subs(&self) -> u32 {
        (0..self.n_beats).map(|b| self.subs(b)).sum()
    }

    #[inline]
    pub fn swing(&self) -> Swing {
        self.swing
    }

    /// Position in beats of subdivision marker `k` of `beat`.
    ///
    /// `k = 0` returns the beat marker itself, exactly — beat marker `k = 0` *is* a
    /// subdivision marker (ADR 0007 §2). `k >= subs(beat)` returns the **next beat
    /// marker**, exactly, rather than `m[b] + 1·span`: `a + (b - a)` is not `b` in
    /// `f64`, and the slot boundaries must agree bit-for-bit with the markers they
    /// sit on or [`Grid::locate`] can land a position in the wrong beat.
    #[inline]
    pub fn sub_pos(&self, beat: usize, k: u32) -> f64 {
        let b = beat.min(self.n_beats - 1);
        let n = self.subs(b);
        if k == 0 {
            return self.markers[b];
        }
        if k >= n {
            return self.markers[b + 1];
        }
        let lo = self.markers[b];
        let hi = self.markers[b + 1];
        lo + self.swing.w(k as f64 / n as f64) * (hi - lo)
    }

    /// Forward mapping: the beat position of a `(beat, sub, frac)` triple, i.e. the
    /// `sub_pos(b, k) + f · (sub_pos(b, k+1) - sub_pos(b, k))` of 0348 (the per-hit
    /// nudge is 0348's, not the grid's). Out-of-range fields clamp.
    pub fn pos_of(&self, at: GridPos) -> f64 {
        let b = at.beat.min(self.n_beats - 1);
        let k = at.sub.min(self.subs(b) - 1);
        let p0 = self.sub_pos(b, k);
        let p1 = self.sub_pos(b, k + 1);
        let f = if at.frac.is_finite() { at.frac } else { 0.0 };
        if f <= 0.0 {
            return p0;
        }
        if f >= 1.0 {
            return p1; // exactly the next marker, so slot ends round-trip
        }
        p0 + f * (p1 - p0)
    }

    /// Inverse mapping: resolve a beat position to its owning `(beat, sub, frac)`.
    /// Positions outside `[0, len_beats]` — and non-finite ones — clamp to the bounds.
    ///
    /// Both scans compare against the very values [`Grid::sub_pos`] produces rather
    /// than inverting the warp analytically, so a position sitting exactly on a marker
    /// resolves to that marker with `frac = 0` for any warp shape, present or future.
    /// Both are bounded (`MAX_BEATS`, then `MAX_SUBS`) and allocation-free.
    pub fn locate(&self, t: f64) -> GridPos {
        let last = self.n_beats - 1;
        if t <= self.markers[0] || t.is_nan() {
            return GridPos { beat: 0, sub: 0, frac: 0.0 };
        }
        if t >= self.markers[self.n_beats] {
            return GridPos { beat: last, sub: self.subs(last) - 1, frac: 1.0 };
        }
        let mut beat = last;
        for i in 0..self.n_beats {
            if t < self.markers[i + 1] {
                beat = i;
                break;
            }
        }
        // The largest k whose marker is at or before t. k = 0 always qualifies
        // (sub_pos(b, 0) == m[b] <= t), so the loop always settles.
        let mut sub = 0;
        for k in (0..self.subs(beat)).rev() {
            if self.sub_pos(beat, k) <= t {
                sub = k;
                break;
            }
        }
        let p0 = self.sub_pos(beat, sub);
        let p1 = self.sub_pos(beat, sub + 1);
        let span = p1 - p0;
        // `span > 0` is guaranteed by MIN_SLOT plus a strictly increasing warp; the
        // guard is belt and braces against a future shape with a flat limb.
        let frac = if span > 0.0 { ((t - p0) / span).clamp(0.0, 1.0) } else { 0.0 };
        GridPos { beat, sub, frac }
    }

    // ── mutation (every path clamps) ──────────────────────────────────────────

    /// Move beat marker `i`, clamped into `(m[i-1] + MIN_SLOT, m[i+1] - MIN_SLOT)`.
    /// Returns the position actually taken.
    ///
    /// A drag that would cross a neighbour **clamps** rather than being rejected: the
    /// marker follows the pointer as far as it legally can, which is what makes a drag
    /// feel continuous. The outer markers are pinned to the pattern bounds and ignore
    /// this entirely — move the end with [`Grid::set_len_beats`]. Non-finite input is
    /// a no-op, so a NaN can never enter the array and poison every later query.
    pub fn set_beat_marker(&mut self, i: usize, pos: f64) -> f64 {
        if i == 0 || i >= self.n_beats {
            return self.beat_marker(i);
        }
        if !pos.is_finite() {
            return self.markers[i];
        }
        let lo = self.markers[i - 1] + MIN_SLOT;
        let hi = self.markers[i + 1] - MIN_SLOT;
        // `hi.max(lo)` cannot bind while the invariant holds (neighbours are at least
        // 2·MIN_SLOT apart); it is here so a degenerate array can never make `clamp`
        // panic on `min > max`, and it prefers the lower bound if it ever does.
        let v = pos.clamp(lo, hi.max(lo));
        self.markers[i] = v;
        v
    }

    /// Set the pattern length — the pinned end marker — rescaling the interior markers
    /// proportionally so the lane's feel survives a length change, then re-establishing
    /// [`MIN_SLOT`]. Returns the length actually taken (at least `n_beats · MIN_SLOT`).
    pub fn set_len_beats(&mut self, len_beats: f64) -> f64 {
        let n = self.n_beats;
        let len = sane_len(len_beats, n);
        let old = self.markers[n];
        if old > 0.0 {
            let k = len / old;
            for m in self.markers.iter_mut().take(n).skip(1) {
                *m *= k;
            }
        }
        for m in self.markers.iter_mut().skip(n) {
            *m = len; // the end marker, then the padding behind it
        }
        self.enforce_min_slot();
        len
    }

    /// Set the beat count, re-laying the markers uniformly over the current length.
    ///
    /// A deliberate rebuild: preserving user marker edits across an insert or delete
    /// (and rubber-banding the hits that hang off them) is 0349's job, and doing half
    /// of it here would leave two different answers in the codebase.
    pub fn set_n_beats(&mut self, n_beats: usize) {
        let n = n_beats.clamp(1, MAX_BEATS);
        let len = sane_len(self.markers[self.n_beats], n);
        let fresh = Self::uniform(n, len, self.default_subs);
        self.markers = fresh.markers;
        self.n_beats = n;
        // Drop overrides on beats that are no longer live, for the same reason the
        // marker tail is canonicalised: a shrink must not leave a value that springs
        // back on a later grow, and two grids with the same live geometry must compare
        // equal however they were built.
        for o in self.sub_override.iter_mut().skip(n) {
            *o = 0;
        }
    }

    /// Set the lane-wide subdivision count (clamped to `1..=MAX_SUBS`).
    pub fn set_default_subs(&mut self, subs: u32) {
        self.default_subs = subs.clamp(1, MAX_SUBS);
    }

    /// Set or clear one beat's subdivision override. `Some(3)` inside an otherwise
    /// 16ths lane is how a triplet is expressed (ADR 0007 §2) — there is no separate
    /// tuplet concept.
    /// Bounded by the **live** beat count, not [`MAX_BEATS`]: an override stored past
    /// the end marker belongs to no beat, is invisible to every query, and would
    /// reappear if the lane later grew.
    pub fn set_beat_subs(&mut self, beat: usize, subs: Option<u32>) {
        if beat < self.n_beats {
            self.sub_override[beat] = subs.map_or(0, |n| n.clamp(1, MAX_SUBS) as u8);
        }
    }

    pub fn set_swing(&mut self, swing: Swing) {
        self.swing = swing;
    }

    /// Re-establish `m[i] - m[i-1] >= MIN_SLOT` across the interior without moving the
    /// pinned outer markers.
    ///
    /// A forward pass pushes markers apart, then a backward pass pulls them off the
    /// pinned end. That pair is sufficient, not merely a heuristic: after the forward
    /// pass `m[i] >= i · MIN_SLOT`, so the backward pass — which sets each marker to at
    /// most its successor minus `MIN_SLOT` — can only ever land a marker at or above
    /// `MIN_SLOT` above its predecessor, given the total span is at least
    /// `n · MIN_SLOT` (which [`sane_len`] guarantees).
    fn enforce_min_slot(&mut self) {
        let n = self.n_beats;
        for i in 1..n {
            let floor = self.markers[i - 1] + MIN_SLOT;
            if self.markers[i] < floor {
                self.markers[i] = floor;
            }
        }
        for i in (1..n).rev() {
            let ceil = self.markers[i + 1] - MIN_SLOT;
            if self.markers[i] > ceil {
                self.markers[i] = ceil;
            }
        }
    }
}

/// A pattern length that can actually hold `n` beat slots: finite, and at least
/// `n · MIN_SLOT`. Below that no marker arrangement satisfies the invariant, so the
/// grid would have to choose between a zero-width slot and a lie — it takes neither.
fn sane_len(len_beats: f64, n: usize) -> f64 {
    let floor = n as f64 * MIN_SLOT;
    if len_beats.is_finite() && len_beats > floor { len_beats } else { floor }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The xorshift32 the codebase already uses (see `LaneState::next_unit`), seeded
    /// explicitly so a property-test failure reproduces exactly.
    struct Rng(u32);

    impl Rng {
        fn next_u32(&mut self) -> u32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.0 = x;
            x
        }
        /// `[0, 1)`.
        fn unit(&mut self) -> f64 {
            (self.next_u32() >> 8) as f64 * (1.0 / 16_777_216.0)
        }
        /// `[-1, 1)`.
        fn bipolar(&mut self) -> f64 {
            self.unit() * 2.0 - 1.0
        }
    }

    const SUB_COUNTS: [u32; 6] = [1, 2, 3, 4, 6, 8];

    // ── swing warp ────────────────────────────────────────────────────────────

    #[test]
    fn swing_tag_round_trips_and_unknown_falls_back() {
        for s in [SwingShape::Straight, SwingShape::Mpc] {
            assert_eq!(SwingShape::from_u8(s.as_u8()), s);
        }
        assert_eq!(SwingShape::from_u8(0xFF), SwingShape::default());
        assert_eq!(SwingShape::default(), SwingShape::Straight);
    }

    #[test]
    fn straight_warp_is_the_identity_exactly() {
        let mut rng = Rng(0x1234_5678);
        for _ in 0..2000 {
            let u = rng.unit();
            assert_eq!(Swing::straight().w(u), u);
            // A nominally-swung lane at amount 0 is bit-identical to a straight one.
            assert_eq!(Swing::mpc(0.0).w(u), u);
        }
    }

    /// AC: `w` monotonic with `w(0) = 0`, `w(1) = 1` across the full swing range.
    #[test]
    fn warp_is_monotonic_with_fixed_endpoints() {
        let mut rng = Rng(0xC0FF_EE01);
        for shape in [SwingShape::Straight, SwingShape::Mpc] {
            for i in 0..64 {
                // Sweep the declared control range plus deliberate overshoot.
                let amount = -1.5 + 3.0 * (i as f64 / 63.0);
                let sw = Swing { shape, amount };
                assert_eq!(sw.w(0.0), 0.0, "w(0) shape={shape:?} a={amount}");
                assert_eq!(sw.w(1.0), 1.0, "w(1) shape={shape:?} a={amount}");
                // Out-of-domain clamps rather than extrapolating.
                assert_eq!(sw.w(-0.5), 0.0);
                assert_eq!(sw.w(2.0), 1.0);
                for _ in 0..400 {
                    let (a, b) = (rng.unit(), rng.unit());
                    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                    if lo == hi {
                        continue;
                    }
                    let (wl, wh) = (sw.w(lo), sw.w(hi));
                    assert!(wl < wh, "not increasing: shape={shape:?} a={amount} {lo}->{wl} {hi}->{wh}");
                    assert!((0.0..=1.0).contains(&wl) && (0.0..=1.0).contains(&wh));
                }
            }
        }
    }

    #[test]
    fn mpc_pulls_the_half_beat_late_and_non_finite_amount_is_straight() {
        assert!(Swing::mpc(1.0).w(0.5) > 0.5);
        assert!(Swing::mpc(-1.0).w(0.5) < 0.5);
        assert_eq!(Swing::mpc(1.0).w(0.5), MPC_MAX_RATIO);
        // Overshoot clamps to the same place, so no caller can flatten or invert w.
        assert_eq!(Swing::mpc(9.0).w(0.5), MPC_MAX_RATIO);
        assert_eq!(Swing::mpc(f64::NAN).w(0.3), 0.3);
        assert_eq!(Swing::mpc(f64::INFINITY).w(0.3), 0.3);
    }

    // ── marker invariants ─────────────────────────────────────────────────────

    fn assert_increasing(g: &Grid) {
        for i in 1..=g.n_beats() {
            let gap = g.beat_marker(i) - g.beat_marker(i - 1);
            assert!(gap > 0.0, "marker {i} not increasing: gap {gap}");
            assert!(gap >= MIN_SLOT * (1.0 - 1e-12), "marker {i} slot too thin: {gap}");
        }
    }

    #[test]
    fn uniform_grid_is_increasing_and_pinned() {
        let g = Grid::uniform(4, 4.0, 4);
        assert_eq!(g.beat_marker(0), 0.0);
        assert_eq!(g.len_beats(), 4.0);
        assert_eq!(g.beat_marker(4), 4.0);
        assert_eq!(g.n_beats(), 4);
        assert_increasing(&g);
        // Out-of-range beat counts / lengths clamp rather than panic.
        assert_eq!(Grid::uniform(0, 4.0, 4).n_beats(), 1);
        assert_eq!(Grid::uniform(999, 4.0, 4).n_beats(), MAX_BEATS);
        assert_eq!(Grid::uniform(4, f64::NAN, 4).len_beats(), 4.0 * MIN_SLOT);
        assert_eq!(Grid::uniform(4, -3.0, 4).len_beats(), 4.0 * MIN_SLOT);
        assert_eq!(Grid::uniform(4, 4.0, 0).default_subs(), 1);
        assert_eq!(Grid::uniform(4, 4.0, 999).default_subs(), MAX_SUBS);
    }

    /// AC: a mutation that would violate strict increase **clamps to `MIN_SLOT`**,
    /// rather than being rejected or accepted.
    #[test]
    fn marker_drag_clamps_to_min_slot() {
        let mut g = Grid::uniform(4, 4.0, 4);
        // Way past the right neighbour → parks one MIN_SLOT short of it.
        let v = g.set_beat_marker(2, 99.0);
        assert_eq!(v, 3.0 - MIN_SLOT);
        assert_eq!(g.beat_marker(2), 3.0 - MIN_SLOT);
        // Way past the left neighbour → one MIN_SLOT beyond it. Note m[1] is still 1.
        let v = g.set_beat_marker(2, -99.0);
        assert_eq!(v, 1.0 + MIN_SLOT);
        assert_increasing(&g);
        // A legal move is taken verbatim.
        assert_eq!(g.set_beat_marker(2, 2.25), 2.25);
        assert_eq!(g.beat_marker(2), 2.25);
    }

    #[test]
    fn outer_markers_are_pinned() {
        let mut g = Grid::uniform(4, 4.0, 4);
        g.set_beat_marker(0, 1.0);
        g.set_beat_marker(4, 9.0);
        assert_eq!(g.beat_marker(0), 0.0);
        assert_eq!(g.beat_marker(4), 4.0);
        // Past the end is not a panic, and reports the end.
        assert_eq!(g.set_beat_marker(40, 9.0), 4.0);
        assert_increasing(&g);
    }

    #[test]
    fn non_finite_drag_is_a_no_op() {
        let mut g = Grid::uniform(4, 4.0, 4);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            g.set_beat_marker(2, bad);
            assert_eq!(g.beat_marker(2), 2.0);
        }
        assert!(g.sub_pos(2, 1).is_finite());
    }

    #[test]
    fn length_change_rescales_and_re_establishes_min_slot() {
        let mut g = Grid::uniform(4, 4.0, 4);
        g.set_beat_marker(1, 1.5);
        // Growing keeps the proportions.
        assert_eq!(g.set_len_beats(8.0), 8.0);
        assert_eq!(g.beat_marker(1), 3.0);
        assert_eq!(g.len_beats(), 8.0);
        assert_increasing(&g);
        // Shrinking to the floor keeps every slot at MIN_SLOT rather than collapsing.
        let floor = 4.0 * MIN_SLOT;
        assert_eq!(g.set_len_beats(0.0), floor);
        assert_eq!(g.len_beats(), floor);
        assert_increasing(&g);
    }

    /// Squeeze a badly skewed marker set through a hard shrink — the pathological case
    /// for the forward/backward sweep, where the forward pass alone would leave the
    /// last slot inverted.
    #[test]
    fn hard_shrink_from_a_skewed_set_stays_valid() {
        let mut rng = Rng(0x5EED_0347);
        for n in 1..=MAX_BEATS {
            let mut g = Grid::uniform(n, 16.0, 4);
            for i in 1..n {
                g.set_beat_marker(i, rng.unit() * 16.0);
            }
            assert_increasing(&g);
            for len in [n as f64 * MIN_SLOT, 0.5, 1.0, 16.0, 0.01] {
                g.set_len_beats(len);
                assert_increasing(&g);
                assert_eq!(g.beat_marker(0), 0.0);
                assert_eq!(g.len_beats(), g.beat_marker(n));
            }
        }
    }

    /// Equality is by geometry, not by construction history — the unused marker
    /// padding must not leak into the derived `PartialEq`.
    #[test]
    fn equal_geometry_compares_equal_however_it_was_built() {
        let mut resized = Grid::uniform(4, 4.0, 4);
        resized.set_len_beats(8.0);
        assert_eq!(resized, Grid::uniform(4, 8.0, 4));

        let mut recounted = Grid::uniform(16, 8.0, 4);
        recounted.set_n_beats(4);
        assert_eq!(recounted, Grid::uniform(4, 8.0, 4));

        let mut swung = Grid::uniform(4, 8.0, 4);
        swung.set_swing(Swing::mpc(0.5));
        assert_ne!(swung, Grid::uniform(4, 8.0, 4));

        // The sub-count overrides are the other half of the geometry and carry the same
        // obligation as the marker tail: an override on a beat that is not live must
        // neither be stored nor leak into equality.
        let mut off_the_end = Grid::uniform(4, 4.0, 4);
        off_the_end.set_beat_subs(9, Some(3));
        assert_eq!(off_the_end, Grid::uniform(4, 4.0, 4));
    }

    /// Shrinking the beat count then growing it back must not resurrect an override
    /// from before the shrink — the beat it belonged to is gone.
    #[test]
    fn beat_count_shrink_drops_overrides_it_passes() {
        let mut g = Grid::uniform(8, 8.0, 4);
        g.set_beat_subs(6, Some(3));
        assert_eq!(g.subs(6), 3);
        g.set_n_beats(4);
        g.set_n_beats(8);
        assert_eq!(g.sub_override(6), None);
        assert_eq!(g.subs(6), 4);
        assert_eq!(g, Grid::uniform(8, 8.0, 4));
    }

    #[test]
    fn beat_count_change_relays_uniformly() {
        let mut g = Grid::uniform(4, 4.0, 4);
        g.set_n_beats(3);
        assert_eq!(g.n_beats(), 3);
        assert_eq!(g.len_beats(), 4.0);
        assert_increasing(&g);
        g.set_n_beats(999);
        assert_eq!(g.n_beats(), MAX_BEATS);
        assert_increasing(&g);
        g.set_n_beats(0);
        assert_eq!(g.n_beats(), 1);
        assert_increasing(&g);
    }

    // ── subdivision geometry ──────────────────────────────────────────────────

    /// AC: sub positions are evenly spaced at zero swing for `n ∈ {1,2,3,4,6,8}`.
    #[test]
    fn zero_swing_subs_are_evenly_spaced() {
        for n in SUB_COUNTS {
            let g = Grid::uniform(4, 4.0, n);
            for b in 0..g.n_beats() {
                assert_eq!(g.subs(b), n);
                assert_eq!(g.sub_pos(b, 0), g.beat_marker(b));
                // k = n is the next beat marker, exactly.
                assert_eq!(g.sub_pos(b, n), g.beat_marker(b + 1));
                for k in 0..n {
                    let want = b as f64 + k as f64 / n as f64;
                    assert!((g.sub_pos(b, k) - want).abs() < 1e-12, "n={n} b={b} k={k}");
                }
            }
        }
    }

    /// AC: sub markers stay strictly increasing at every swing amount for every `n`.
    #[test]
    fn subs_strictly_increase_at_every_swing_amount() {
        let mut rng = Rng(0x0347_0007);
        for n in SUB_COUNTS {
            for i in 0..48 {
                let amount = -1.25 + 2.5 * (i as f64 / 47.0);
                let mut g = Grid::uniform(4, 4.0, n);
                g.set_swing(Swing::mpc(amount));
                // Skew the markers too, so this is not just the uniform case.
                g.set_beat_marker(1, 0.3 + rng.unit());
                g.set_beat_marker(2, 1.6 + rng.unit());
                g.set_beat_marker(3, 2.9 + rng.unit());
                for b in 0..g.n_beats() {
                    let mut prev = g.sub_pos(b, 0);
                    assert_eq!(prev, g.beat_marker(b));
                    for k in 1..=n {
                        let p = g.sub_pos(b, k);
                        assert!(p > prev, "n={n} a={amount} b={b} k={k}: {prev} -> {p}");
                        prev = p;
                    }
                    assert_eq!(prev, g.beat_marker(b + 1));
                }
            }
        }
    }

    /// AC: zero swing on a straight marker set reproduces the old uniform grid to
    /// **`f64` equality**.
    ///
    /// Dyadic sub-counts only. For `n = 3` the two expressions are genuinely different
    /// roundings of the same real — `m[b] + k/n` rounds the fraction once and adds an
    /// exact integer, while `i · step_beats` rounds `1/n` and then rounds the product —
    /// and they differ by 1 ULP at some `i` (e.g. `2 + 1/3` vs `7 · (1/3)`). See
    /// `non_dyadic_sub_counts_match_the_old_grid_to_one_ulp`; the old form is the one
    /// carrying the accumulated error, so this is not a regression.
    #[test]
    fn zero_swing_reproduces_the_uniform_grid_exactly() {
        for n in [1u32, 2, 4, 8, 16] {
            let step_beats = 1.0 / n as f64;
            let g = Grid::uniform(MAX_BEATS, MAX_BEATS as f64, n);
            for b in 0..g.n_beats() {
                for k in 0..n {
                    let i = (b as u32 * n + k) as f64;
                    assert_eq!(
                        g.sub_pos(b, k),
                        i * step_beats,
                        "n={n} b={b} k={k} must be bit-exact"
                    );
                }
            }
        }
        // The step model's own divisors, at their own beat counts.
        for (n, step) in [(4u32, crate::sequencer::SIXTEENTH), (2, crate::sequencer::EIGHTH)] {
            let g = Grid::uniform(4, 4.0, n);
            for b in 0..4 {
                for k in 0..n {
                    assert_eq!(g.sub_pos(b, k), (b as u32 * n + k) as f64 * step);
                }
            }
        }
    }

    #[test]
    fn non_dyadic_sub_counts_match_the_old_grid_to_one_ulp() {
        for n in [3u32, 6, 12] {
            let step_beats = 1.0 / n as f64;
            let g = Grid::uniform(MAX_BEATS, MAX_BEATS as f64, n);
            for b in 0..g.n_beats() {
                for k in 0..n {
                    let want = (b as u32 * n + k) as f64 * step_beats;
                    let got = g.sub_pos(b, k);
                    // 1 ULP at beat 16 is ~2e-15 beats — nanoseconds at any tempo.
                    assert!((got - want).abs() <= 4.0 * f64::EPSILON * want.max(1.0), "n={n} b={b} k={k}: {got} vs {want}");
                }
            }
        }
    }

    /// AC: a single `n = 3` beat inside an `n = 4` lane places three evenly-spaced
    /// subs in that beat and four everywhere else.
    #[test]
    fn per_beat_sub_override_places_a_triplet() {
        let mut g = Grid::uniform(4, 4.0, 4);
        g.set_beat_subs(2, Some(3));
        assert_eq!(g.sub_override(2), Some(3));
        assert_eq!(g.sub_override(1), None);
        assert_eq!(g.subs(2), 3);
        for b in [0, 1, 3] {
            assert_eq!(g.subs(b), 4);
            for k in 0..4 {
                assert_eq!(g.sub_pos(b, k), b as f64 + k as f64 * 0.25);
            }
        }
        for k in 0..3 {
            let want = 2.0 + k as f64 / 3.0;
            assert!((g.sub_pos(2, k) - want).abs() < 1e-12, "k={k}");
        }
        assert_eq!(g.sub_pos(2, 3), 3.0);
        assert_eq!(g.total_subs(), 4 + 4 + 3 + 4);
        // Clearing the override falls back to the lane default.
        g.set_beat_subs(2, None);
        assert_eq!(g.subs(2), 4);
        assert_eq!(g.sub_override(2), None);
        // Overrides clamp like the default does, and out-of-range beats are a no-op.
        g.set_beat_subs(0, Some(0));
        assert_eq!(g.subs(0), 1);
        g.set_beat_subs(0, Some(999));
        assert_eq!(g.subs(0), MAX_SUBS);
        g.set_beat_subs(MAX_BEATS + 5, Some(3));
        assert_eq!(g.subs(1), 4);
    }

    // ── forward / inverse mapping ─────────────────────────────────────────────

    #[test]
    fn locate_lands_on_markers_with_zero_fraction() {
        let mut g = Grid::uniform(4, 4.0, 4);
        g.set_swing(Swing::mpc(0.6));
        for b in 0..g.n_beats() {
            for k in 0..g.subs(b) {
                let p = g.sub_pos(b, k);
                let at = g.locate(p);
                assert_eq!(at.beat, b, "pos {p}");
                assert_eq!(at.sub, k, "pos {p}");
                assert_eq!(at.frac, 0.0, "pos {p}");
                assert_eq!(g.pos_of(at), p);
            }
        }
    }

    #[test]
    fn locate_clamps_outside_the_pattern() {
        let g = Grid::uniform(4, 4.0, 4);
        for t in [-1.0, 0.0, f64::NAN, f64::NEG_INFINITY] {
            assert_eq!(g.locate(t), GridPos { beat: 0, sub: 0, frac: 0.0 }, "t={t}");
        }
        for t in [4.0, 9.0, f64::INFINITY] {
            assert_eq!(g.locate(t), GridPos { beat: 3, sub: 3, frac: 1.0 }, "t={t}");
        }
        // The end resolves to the last slot's end, which is the pattern end.
        assert_eq!(g.pos_of(g.locate(4.0)), 4.0);
    }

    #[test]
    fn pos_of_clamps_out_of_range_fields() {
        let g = Grid::uniform(4, 4.0, 4);
        let end = GridPos { beat: 99, sub: 99, frac: 2.0 };
        assert_eq!(g.pos_of(end), 4.0);
        let start = GridPos { beat: 0, sub: 0, frac: -1.0 };
        assert_eq!(g.pos_of(start), 0.0);
        let nan = GridPos { beat: 1, sub: 1, frac: f64::NAN };
        assert_eq!(g.pos_of(nan), g.sub_pos(1, 1));
    }

    /// AC: forward-then-inverse round-trips for randomised positions inside the bounds.
    #[test]
    fn mapping_round_trips_for_random_positions() {
        let mut rng = Rng(0xBEEF_0347);
        for n in SUB_COUNTS {
            for shape in [SwingShape::Straight, SwingShape::Mpc] {
                for trial in 0..24 {
                    let mut g = Grid::uniform(4, 4.0, n);
                    g.set_swing(Swing { shape, amount: rng.bipolar() });
                    g.set_beat_marker(1, 0.4 + rng.unit() * 0.8);
                    g.set_beat_marker(2, 1.6 + rng.unit() * 0.8);
                    g.set_beat_marker(3, 2.7 + rng.unit() * 0.8);
                    g.set_beat_subs(trial % 4, Some(3)); // a tuplet in the mix
                    for _ in 0..400 {
                        let t = rng.unit() * g.len_beats();
                        let at = g.locate(t);
                        assert!(at.beat < g.n_beats());
                        assert!(at.sub < g.subs(at.beat));
                        assert!((0.0..=1.0).contains(&at.frac), "frac {} out of range", at.frac);
                        let back = g.pos_of(at);
                        assert!(
                            (back - t).abs() < 1e-12,
                            "n={n} shape={shape:?} t={t} -> {at:?} -> {back}"
                        );
                        // And the slot really does own t.
                        assert!(g.sub_pos(at.beat, at.sub) <= t);
                        assert!(t <= g.sub_pos(at.beat, at.sub + 1));
                    }
                }
            }
        }
    }

    #[test]
    fn round_trip_survives_a_single_beat_grid() {
        let mut rng = Rng(0x0001_0347);
        let mut g = Grid::uniform(1, 1.0, 1);
        g.set_swing(Swing::mpc(0.7));
        assert_eq!(g.total_subs(), 1);
        assert_eq!(g.sub_pos(0, 0), 0.0);
        assert_eq!(g.sub_pos(0, 1), 1.0);
        for _ in 0..500 {
            let t = rng.unit();
            let at = g.locate(t);
            assert_eq!((at.beat, at.sub), (0, 0));
            assert!((g.pos_of(at) - t).abs() < 1e-12);
        }
    }

    #[test]
    fn default_grid_is_the_old_sixteen_step_lane() {
        let g = Grid::default();
        assert_eq!(g.n_beats(), 4);
        assert_eq!(g.len_beats(), 4.0);
        assert_eq!(g.total_subs(), 16);
        assert_eq!(g.swing(), Swing::straight());
        for i in 0..16u32 {
            let b = (i / 4) as usize;
            assert_eq!(g.sub_pos(b, i % 4), i as f64 * crate::sequencer::SIXTEENTH);
        }
    }
}
