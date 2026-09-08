//! Lane timing **geometry** — beat markers, derived subdivision markers, and the
//! swing warp that positions the latter (ADR 0007 §2/§3, ticket 0347).
//!
//! Two tiers of marker. **Beat markers** `m[0..=n]` are stored and user-draggable;
//! **subdivision markers** are never stored, they are computed on demand. The beat is
//! tiled by swing *periods* of `c` subdivisions ([`SwingPeriod`]) and `w` is applied
//! inside each one:
//!
//! ```text
//! g     = ⌊k / c⌋ · c            (first subdivision of k's period)
//! width = min(c, n_b - g)        (a trailing period can be short)
//! sub_pos(b, k) = m[b] + (g + width · w((k - g) / width)) / n_b · (m[b+1] - m[b])
//! ```
//!
//! `n_b` is the beat's subdivision count — a lane default with a per-beat override,
//! which is where tuplets live: one beat with `n = 3` inside an otherwise-16ths lane,
//! no separate concept and no special case. `w` is the swing warp: monotonic on
//! `[0, 1]` with `w(0) = 0`, `w(1) = 1`. Beat marker `k = 0` *is* a subdivision
//! marker, so the snap-target set is exactly the subdivision markers.
//!
//! Swing is a **warp on a unit interval** rather than a per-position offset table for
//! one reason (ADR 0007 §3): it generalises over `n_b`. One swing control stays
//! meaningful whatever a beat's sub-count, `n = 3` needs no special case, and derived
//! markers cannot drift out of step with the beat markers that generate them. Which
//! interval it warps is the ADR 0007 Amendment / ticket 0365 correction: `w` has one
//! knee, so applying it across the whole beat swings the beat rather than the
//! subdivisions — at `n = 4` long-long-short-short rather than shuffle, and at every
//! other `n > 2` some other rhythm that is not the one asked for.
//!
//! **[`MIN_SLOT`] is load-bearing, not cosmetic.** Ticket 0348 stores a hit as
//! `t = sub_pos(b, k) + f · (sub_pos(b, k+1) - sub_pos(b, k)) + nudge`; a zero-width
//! slot makes `f` unresolvable and divides by ~0, silently poisoning downstream hits
//! with NaN. Every mutation path here therefore goes through the clamp, and there is
//! no `&mut` into the marker array to bypass it. [`MAX_LEN_BEATS`] is the other half
//! of that guarantee: `MIN_SLOT` is absolute, so a clamp against it only separates
//! two markers while it is larger than an ulp of them.
//!
//! Pure data and math: no scheduler, no UI, **no allocation on any query path**, and
//! storage is fixed-capacity arrays sized from [`MAX_BEATS`] / [`MAX_SUBS`] so the
//! whole grid is `Copy` and audio-thread safe. Marker *editing* semantics (a drag
//! rubber-bands the hits it owns, insert/delete preserves absolute time) are 0349;
//! this module owns the geometry and its invariants only.
//!
//! This is the coordinate system [`crate::sequencer::Pattern`] stores its hits
//! against (0348). The ADR 0001 §2 polymeter the old `len`/`step_beats` pair
//! provided is not lost — marker sets are per-lane, which is what preserves it.

/// Maximum beat slots in one lane's grid (storage ceiling; the live count may be
/// shorter, exactly as [`crate::sequencer::MAX_HITS`] ceilings a lane's hit list).
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

/// Maximum pattern length in beats — 2¹⁶, about nine hours of 4/4 at 120 bpm and
/// four thousand times the longest lane [`MAX_BEATS`] allows.
///
/// A ceiling rather than a taste. Two absolute quantities are compared against
/// marker positions, and both stop meaning anything once a marker's ulp overtakes
/// them:
///
/// - [`MIN_SLOT`], past `MIN_SLOT · 2⁵³ ≈ 2⁴⁸` beats. `m ± MIN_SLOT` is then `m`, so
///   [`Grid::set_beat_marker`]'s clamp parks a dragged marker exactly *on* its
///   neighbour and leaves the zero-width slot [`Grid::locate`] divides by.
/// - The `f32` a hit's in-slot fraction is stored in, past `2¹⁸`ish beats. Once
///   `ulp(m)` exceeds an `f32` ulp of the narrowest slot, re-deriving a fraction no
///   longer round-trips, and 0349's "an untouched slot keeps its triple bit-for-bit"
///   stops holding.
///
/// The constant sits below **both**, with margin — the assertion below is the second
/// and tighter one, written out rather than trusted. Everything in this module and
/// in [`crate::sequencer`] that argues "the clamp keeps this slot positive" or "this
/// fraction round-trips" is really arguing from here (adversarial review, 0349).
///
/// A power of two, so a length change stays an exact rescale.
pub const MAX_LEN_BEATS: f64 = (1u32 << 16) as f64;
const _: () = assert!(
    MAX_LEN_BEATS * f64::EPSILON < (MIN_SLOT / MAX_SUBS as f64) * f32::EPSILON as f64
);

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

/// Width of one swing period — the interval `w` is applied *to*, in subdivisions
/// (ADR 0007 Amendment, ticket 0365).
///
/// `w` has fixed endpoints, so whatever it spans gets exactly one knee. Spanning the
/// whole beat that is one knee per beat, which is classic shuffle only at `n = 2` and
/// a back-loaded beat everywhere else; spanning a pair it is classic shuffle at every
/// even `n`. So the span is a control rather than a constant: on a 16ths lane
/// [`SwingPeriod::Beat`] is 8th-note swing and [`SwingPeriod::Pair`] is 16th shuffle.
///
/// **A period that does not divide the beat's sub-count leaves a short trailing
/// group, warped across its own width** — and that is the whole of the odd-`n`
/// answer. At `n = 3` with `Pair` the leftover third subdivision is a group of one,
/// so `w(0) = 0` places it unswung on its own boundary; the first two shuffle
/// normally. Odd `n` is therefore a setting like any other rather than a second rule
/// inside [`Grid::sub_pos`], and a triplet lane that wants the beat-wide feel asks
/// for `Beat`. The same rule bounds every period: a group is warped across the
/// subdivisions it actually has, so no subdivision can be pushed past its beat marker.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum SwingPeriod {
    /// The whole beat — one knee per beat, whatever the sub-count. The pre-0365
    /// behaviour, kept because it *is* the wanted feel one subdivision level up.
    Beat,
    /// Two subdivisions: classic shuffle, and the default.
    #[default]
    Pair,
    /// An arbitrary period, in subdivisions, clamped to `1..=MAX_SUBS`. A period of 1
    /// is no swing at all. `Custom(2)` is [`SwingPeriod::Pair`] and canonicalises to
    /// it on the way into a [`Grid`], so two grids with the same feel compare equal.
    Custom(u8),
}

impl SwingPeriod {
    #[inline]
    pub fn as_u8(self) -> u8 {
        match self {
            SwingPeriod::Beat => 0,
            SwingPeriod::Pair => 2,
            SwingPeriod::Custom(c) => c.clamp(1, MAX_SUBS as u8),
        }
    }

    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => SwingPeriod::Beat,
            2 => SwingPeriod::Pair,
            c if c as u32 <= MAX_SUBS => SwingPeriod::Custom(c),
            _ => SwingPeriod::default(),
        }
    }

    /// Subdivisions in one period, for a beat of `n`. Never zero and never wider than
    /// the beat: a period that straddled a beat marker would put a subdivision in a
    /// slot the marker does not own.
    #[inline]
    pub fn subs(self, n: u32) -> u32 {
        let c = match self {
            SwingPeriod::Beat => n,
            SwingPeriod::Pair => 2,
            SwingPeriod::Custom(c) => c as u32,
        };
        c.clamp(1, n.max(1))
    }
}

/// Ratio the [`SwingShape::Mpc`] knee reaches at `amount = 1`: the period's midpoint
/// lands 75% of the way through it, the classic MPC ceiling. A negative amount mirrors
/// it to 25% (pull early), so `s` never leaves `(0, 1)` and the warp stays strictly
/// increasing across the whole control range.
const MPC_MAX_RATIO: f64 = 0.75;

/// The swing warp: a shape, its amount, and the period it is applied over. `amount` is
/// a bipolar `-1..1` control — positive pulls the late half of each period later (the
/// usual direction), negative early, zero straight. Out-of-range and non-finite values
/// are treated as their clamp, so no caller can produce a non-monotonic `w`.
#[derive(Copy, Clone, Debug, PartialEq, Default)]
pub struct Swing {
    pub shape: SwingShape,
    pub amount: f64,
    pub period: SwingPeriod,
}

impl Swing {
    /// No swing — `w` is the identity.
    #[inline]
    pub fn straight() -> Self {
        Self { shape: SwingShape::Straight, amount: 0.0, period: SwingPeriod::Pair }
    }

    /// Classic MPC swing at `amount` (`-1..1`, positive = late), over the default
    /// pair period.
    #[inline]
    pub fn mpc(amount: f64) -> Self {
        Self { shape: SwingShape::Mpc, amount, period: SwingPeriod::Pair }
    }

    /// The same swing over a different period.
    #[inline]
    pub fn with_period(self, period: SwingPeriod) -> Self {
        Self { period, ..self }
    }

    /// The warp `w: [0, 1] → [0, 1]`, over one swing period's unit interval.
    /// **Strictly increasing with fixed endpoints** `w(0) = 0`, `w(1) = 1` — the two
    /// properties [`Grid::sub_pos`] relies on to keep subdivision markers strictly
    /// increasing and inside their beat, whatever the sub-count or period.
    ///
    /// `w` takes no sub-count and no period: it is the shape of one knee, and where
    /// that knee is applied is [`SwingPeriod`]'s business alone.
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
                // Knee position: where the period's midpoint lands. s ∈ [0.25, 0.75],
                // so both limbs have positive slope (2s and 2(1-s)) and w stays
                // strictly increasing. At amount = 0 both limbs collapse to the
                // identity *exactly* — every operation below is a dyadic scale or a
                // Sterbenz subtraction, so a nominally-Mpc lane at zero swing is
                // bit-identical to a straight one.
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
    ///
    /// The warp spans one [`SwingPeriod`], not the beat (ADR 0007 Amendment): `w` has
    /// a single knee, so a beat-wide application pulls everything past its midpoint —
    /// the on-beat 8th included — and gives long-long-short-short rather than shuffle.
    /// One knee per period is what makes long-short-long-short at every even `n`.
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
        let c = self.swing.period.subs(n);
        let g = k / c * c;
        // A trailing period can be short, and is warped across the subdivisions it
        // actually has. That is what makes the geometry hold with no divisibility
        // assumption: `u < 1` strictly, so `w(u) < 1`, so this group's last marker
        // lands below `g + width` — the next group's first, and for the final group
        // the beat's own end. Strictly increasing and inside the beat for any `c`, `n`.
        let width = c.min(n - g);
        let u = (k - g) as f64 / width as f64;
        // One division, of the whole numerator by `n`. At zero swing that numerator is
        // `g + width · (k - g)/width`, which is exactly `k` for every width a lane can
        // reach — see `a_width_never_loses_the_subdivision_it_divides_out`, which pins
        // that to `MAX_SUBS` rather than to dyadic widths. So a straight lane still
        // reproduces the uniform grid bit-for-bit whatever the period.
        lo + ((g as f64 + width as f64 * self.swing.w(u)) / n as f64) * (hi - lo)
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

    /// Insert a beat marker at `pos`, taking index `i` — splitting beat slot `i - 1`
    /// in two. Returns the index taken, or `None` if the insert is refused.
    ///
    /// Refused when the lane is already at [`MAX_BEATS`]; when `i` is not an interior
    /// index (`1..=n_beats`) or `pos` is outside the pattern, both of which would
    /// unpin an outer marker; when `pos` is non-finite; and when the slot being split
    /// is too narrow to yield two of [`MIN_SLOT`]. A refused insert leaves the grid
    /// bit-for-bit as it was.
    ///
    /// `pos` is written through [`Grid::set_beat_marker`], so it takes exactly the
    /// clamp a drag takes and there is no second path into the marker array.
    ///
    /// The two halves inherit the sub-count of the beat they were cut from, so
    /// splitting a tuplet beat gives two tuplet beats rather than silently reverting
    /// to the lane default.
    ///
    /// **Geometry only.** The hits hanging off the grid keep their *absolute* times
    /// across an insert (ADR 0007 §5); that is
    /// [`crate::sequencer::Pattern::insert_beat_marker`]'s half, and reaching this
    /// through `edit_grid` instead applies the relative rule a *drag* follows — the
    /// opposite gesture.
    pub fn insert_beat_marker(&mut self, i: usize, pos: f64) -> Option<usize> {
        if self.n_beats >= MAX_BEATS || i == 0 || i > self.n_beats || !pos.is_finite() {
            return None;
        }
        if pos <= self.markers[0] || pos >= self.markers[self.n_beats] {
            return None;
        }
        // Splitting a slot narrower than two MIN_SLOTs could only produce a slot
        // thinner than the minimum, which is the one thing the clamp exists to stop.
        if self.markers[i] - self.markers[i - 1] < 2.0 * MIN_SLOT {
            return None;
        }
        for j in (i..=self.n_beats).rev() {
            self.markers[j + 1] = self.markers[j];
        }
        for j in (i..self.n_beats).rev() {
            self.sub_override[j + 1] = self.sub_override[j];
        }
        self.sub_override[i] = self.sub_override[i - 1];
        self.n_beats += 1;
        self.markers[i] = self.markers[i - 1]; // overwritten by the clamped write below
        self.set_beat_marker(i, pos);
        // The width test above is in beats and the clamp is in floats: at an absurd
        // pattern length `m + MIN_SLOT` rounds back to `m`, and a zero-width slot is
        // what makes 0348's inverse mapping divide by ~0. Checking the result rather
        // than trusting the precondition is what makes that unreachable.
        if self.markers[i] <= self.markers[i - 1] || self.markers[i + 1] <= self.markers[i] {
            self.drop_marker(i);
            return None;
        }
        self.canonicalise_tail();
        Some(i)
    }

    /// Delete beat marker `i`, merging slots `i - 1` and `i` into one. Returns whether
    /// it was deleted: the outer markers are the pattern bounds and are refused, which
    /// also means a one-beat lane has nothing to delete.
    ///
    /// The merged slot keeps the **left** beat's sub-count, the one whose marker
    /// survives. A delete can only widen a slot, so no clamp can bind here.
    ///
    /// Geometry only, exactly as [`Grid::insert_beat_marker`] — hits keep their
    /// absolute times through [`crate::sequencer::Pattern::delete_beat_marker`].
    pub fn delete_beat_marker(&mut self, i: usize) -> bool {
        if i == 0 || i >= self.n_beats {
            return false;
        }
        self.drop_marker(i);
        true
    }

    /// Shift marker `i` and every sub-count override past it down one place. The body
    /// of a delete, and the exact inverse of the shift an insert makes — which is what
    /// lets a refused insert roll itself back.
    fn drop_marker(&mut self, i: usize) {
        for j in i..self.n_beats {
            self.markers[j] = self.markers[j + 1];
        }
        for j in i..self.n_beats - 1 {
            self.sub_override[j] = self.sub_override[j + 1];
        }
        self.n_beats -= 1;
        self.canonicalise_tail();
    }

    /// Hold the storage past the live geometry at its canonical value, so two grids
    /// with the same geometry compare equal however they were built: marker padding
    /// equal to the end marker, no override on a beat that is not live.
    fn canonicalise_tail(&mut self) {
        let end = self.markers[self.n_beats];
        for m in self.markers.iter_mut().skip(self.n_beats + 1) {
            *m = end;
        }
        for o in self.sub_override.iter_mut().skip(self.n_beats) {
            *o = 0;
        }
    }

    /// Set the beat count, re-laying the markers uniformly over the current length.
    ///
    /// A deliberate rebuild, and the *other* answer to a change of beat count: this
    /// one throws the marker positions away and re-lays them evenly, which is what a
    /// "4 beats, not 3" control means. [`Grid::insert_beat_marker`] and
    /// [`Grid::delete_beat_marker`] are the marker-preserving pair, where every other
    /// marker holds its position and only one slot changes shape.
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

    /// Set the swing warp. The period is canonicalised through its tag encoding on the
    /// way in — same obligation as the marker tail and the dead sub-count overrides:
    /// `Custom(2)` *is* `Pair`, and an out-of-range `Custom` is its clamp, so two grids
    /// spelling one feel two ways must compare equal.
    ///
    /// Only the spellings that are equal for **every** sub-count are folded.
    /// [`SwingPeriod::Beat`] and `Custom(n)` agree on a lane where every beat has `n`
    /// subs and diverge the moment one beat overrides its count, so they stay distinct.
    pub fn set_swing(&mut self, swing: Swing) {
        self.swing = swing.with_period(SwingPeriod::from_u8(swing.period.as_u8()));
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

/// A pattern length that can actually hold `n` beat slots: finite, at least
/// `n · MIN_SLOT`, and at most [`MAX_LEN_BEATS`]. Below the floor no marker
/// arrangement satisfies the invariant, so the grid would have to choose between a
/// zero-width slot and a lie — it takes neither. Above the ceiling the invariant
/// stops meaning anything, because `MIN_SLOT` falls below one ulp of a marker.
///
/// This is the single gate on the marker array's magnitude: every position in the
/// grid lies in `[0, len]`, so capping the length here is what makes the `MIN_SLOT`
/// arithmetic exact everywhere else in the module.
fn sane_len(len_beats: f64, n: usize) -> f64 {
    let floor = n as f64 * MIN_SLOT;
    if len_beats.is_finite() && len_beats > floor {
        len_beats.min(MAX_LEN_BEATS)
    } else {
        floor
    }
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
        /// `[0, n)`.
        fn below(&mut self, n: u32) -> u32 {
            self.next_u32() % n.max(1)
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
                let sw = Swing { shape, amount, ..Swing::default() };
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

    /// `w` knows nothing of beats or sub-counts: it is one knee on `[0, 1]`, and what
    /// that interval *is* — beat, pair, or custom — is [`SwingPeriod`]'s business.
    #[test]
    fn mpc_pulls_the_period_midpoint_late_and_non_finite_amount_is_straight() {
        assert!(Swing::mpc(1.0).w(0.5) > 0.5);
        assert!(Swing::mpc(-1.0).w(0.5) < 0.5);
        assert_eq!(Swing::mpc(1.0).w(0.5), MPC_MAX_RATIO);
        // Overshoot clamps to the same place, so no caller can flatten or invert w.
        assert_eq!(Swing::mpc(9.0).w(0.5), MPC_MAX_RATIO);
        assert_eq!(Swing::mpc(f64::NAN).w(0.3), 0.3);
        assert_eq!(Swing::mpc(f64::INFINITY).w(0.3), 0.3);
        // Changing the period cannot change w — that is the whole point of the split.
        for p in [SwingPeriod::Beat, SwingPeriod::Pair, SwingPeriod::Custom(5)] {
            assert_eq!(Swing::mpc(1.0).with_period(p).w(0.5), MPC_MAX_RATIO);
        }
    }

    #[test]
    fn swing_period_tag_round_trips_and_unknown_falls_back() {
        assert_eq!(SwingPeriod::default(), SwingPeriod::Pair);
        for p in [SwingPeriod::Beat, SwingPeriod::Pair, SwingPeriod::Custom(1), SwingPeriod::Custom(3), SwingPeriod::Custom(MAX_SUBS as u8)] {
            assert_eq!(SwingPeriod::from_u8(p.as_u8()), p);
        }
        assert_eq!(SwingPeriod::from_u8(0xFF), SwingPeriod::default());
        // Custom(2) is a pair however it is spelled, and out-of-range customs clamp
        // into the representable range rather than falling back to the default.
        assert_eq!(SwingPeriod::from_u8(SwingPeriod::Custom(2).as_u8()), SwingPeriod::Pair);
        assert_eq!(SwingPeriod::from_u8(SwingPeriod::Custom(0).as_u8()), SwingPeriod::Custom(1));
        assert_eq!(SwingPeriod::from_u8(SwingPeriod::Custom(99).as_u8()), SwingPeriod::Custom(MAX_SUBS as u8));
    }

    /// A period is never zero and never wider than the beat, so a group cannot straddle
    /// a beat marker whatever a caller asks for.
    #[test]
    fn period_width_clamps_into_the_beat() {
        for n in SUB_COUNTS {
            assert_eq!(SwingPeriod::Beat.subs(n), n);
            assert_eq!(SwingPeriod::Pair.subs(n), 2.min(n));
            assert_eq!(SwingPeriod::Custom(0).subs(n), 1);
            assert_eq!(SwingPeriod::Custom(99).subs(n), n);
            assert!((1..=n).contains(&SwingPeriod::Custom(5).subs(n)));
        }
    }

    /// The grid canonicalises the period, for the same reason it canonicalises the
    /// marker tail: equal geometry must compare equal however it was built.
    #[test]
    fn set_swing_canonicalises_the_period() {
        let mut spelled = Grid::uniform(4, 4.0, 4);
        spelled.set_swing(Swing::mpc(0.5).with_period(SwingPeriod::Custom(2)));
        let mut named = Grid::uniform(4, 4.0, 4);
        named.set_swing(Swing::mpc(0.5).with_period(SwingPeriod::Pair));
        assert_eq!(spelled, named);
        assert_eq!(spelled.swing().period, SwingPeriod::Pair);
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

    // ── marker insert / delete (0349) ─────────────────────────────────────────

    /// AC: an insert splits one slot and leaves every other marker exactly where it
    /// was — the whole point of the pair, against [`Grid::set_n_beats`]'s rebuild.
    #[test]
    fn insert_splits_one_slot_and_moves_no_other_marker() {
        let mut g = Grid::uniform(4, 4.0, 4);
        g.set_beat_marker(1, 0.75);
        let before: [f64; 5] = std::array::from_fn(|i| g.beat_marker(i));

        assert_eq!(g.insert_beat_marker(2, 1.5), Some(2));
        assert_eq!(g.n_beats(), 5);
        assert_eq!(g.len_beats(), 4.0);
        assert_eq!(g.beat_marker(2), 1.5);
        // Everything below the split is where it was; everything above it shifted
        // index but not position.
        assert_eq!(g.beat_marker(0), before[0]);
        assert_eq!(g.beat_marker(1), before[1]);
        for (i, m) in before.iter().enumerate().skip(2) {
            assert_eq!(g.beat_marker(i + 1), *m, "marker {i} moved");
        }
        assert_increasing(&g);

        // And deleting it again puts the geometry back, exactly.
        assert!(g.delete_beat_marker(2));
        assert_eq!(g.n_beats(), 4);
        for (i, m) in before.iter().enumerate() {
            assert_eq!(g.beat_marker(i), *m, "marker {i} after delete");
        }
        assert_increasing(&g);
    }

    /// AC: outer markers reject insert-outside and delete. They are the pattern
    /// bounds — a hit before `m[0]` would have no owning slot.
    #[test]
    fn outer_markers_reject_insert_and_delete() {
        let mut g = Grid::uniform(4, 4.0, 4);
        let before = g;
        // Index 0 is before the pinned start; past `n_beats` is past the pinned end.
        assert_eq!(g.insert_beat_marker(0, 0.5), None);
        assert_eq!(g.insert_beat_marker(5, 3.5), None);
        assert_eq!(g.insert_beat_marker(99, 3.5), None);
        // A position outside the pattern is refused whatever index is asked for.
        for (i, pos) in [(1, 0.0), (1, -1.0), (4, 4.0), (4, 9.0)] {
            assert_eq!(g.insert_beat_marker(i, pos), None, "i={i} pos={pos}");
        }
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(g.insert_beat_marker(2, bad), None);
        }
        assert!(!g.delete_beat_marker(0), "the start marker is pinned");
        assert!(!g.delete_beat_marker(4), "the end marker is pinned");
        assert!(!g.delete_beat_marker(99));
        assert_eq!(g, before, "a refused edit changes nothing");

        // A one-beat lane has only outer markers, so nothing to delete at all.
        let mut one = Grid::uniform(1, 1.0, 4);
        assert!(!one.delete_beat_marker(1));
        assert_eq!(one.n_beats(), 1);
    }

    /// AC: no path can write a marker position that bypasses the [`MIN_SLOT`] clamp —
    /// an insert goes through the very write a drag does, and a slot too narrow to
    /// split is refused rather than halved.
    #[test]
    fn insert_takes_the_drag_clamp_and_refuses_an_unsplittable_slot() {
        let mut g = Grid::uniform(2, 2.0, 4);
        assert_eq!(g.insert_beat_marker(1, 99.0), None, "outside the pattern");
        // Just short of the right-hand marker: parks one MIN_SLOT off it, as a drag does.
        assert_eq!(g.insert_beat_marker(1, 1.0 - 1e-9), Some(1));
        assert_eq!(g.beat_marker(1), 1.0 - MIN_SLOT);
        assert_increasing(&g);

        // A slot exactly two MIN_SLOTs wide splits into two of exactly MIN_SLOT; one
        // hair narrower is refused rather than yielding a slot below the minimum.
        let mut tight = Grid::uniform(1, 2.0 * MIN_SLOT, 4);
        assert_eq!(tight.insert_beat_marker(1, MIN_SLOT), Some(1));
        assert_eq!(tight.beat_marker(1), MIN_SLOT);
        assert_increasing(&tight);
        let mut too_tight = Grid::uniform(1, 2.0 * MIN_SLOT - 1e-9, 4);
        assert_eq!(too_tight.insert_beat_marker(1, MIN_SLOT), None);
        assert_eq!(too_tight.n_beats(), 1);
    }

    #[test]
    fn insert_stops_at_the_beat_ceiling() {
        let mut full = Grid::uniform(MAX_BEATS, MAX_BEATS as f64, 4);
        let before = full;
        assert_eq!(full.insert_beat_marker(1, 0.5), None);
        assert_eq!(full, before);
        // One below the ceiling still takes it, and lands exactly on it.
        let mut g = Grid::uniform(MAX_BEATS - 1, MAX_BEATS as f64, 4);
        assert_eq!(g.insert_beat_marker(1, 0.5), Some(1));
        assert_eq!(g.n_beats(), MAX_BEATS);
        assert_increasing(&g);
    }

    /// The split halves inherit the sub-count of the beat they came from, and a merge
    /// keeps the surviving marker's. Every other override rides its own beat.
    #[test]
    fn insert_and_delete_carry_the_sub_count_overrides() {
        let mut g = Grid::uniform(4, 4.0, 4);
        g.set_beat_subs(1, Some(3));
        g.set_beat_subs(3, Some(6));
        assert_eq!(g.insert_beat_marker(2, 1.5), Some(2));
        assert_eq!(g.sub_override(0), None);
        assert_eq!(g.sub_override(1), Some(3), "the split beat keeps its count");
        assert_eq!(g.sub_override(2), Some(3), "and so does the half cut off it");
        assert_eq!(g.sub_override(3), None, "old beat 2 rode up one index");
        assert_eq!(g.sub_override(4), Some(6));

        // Deleting the marker again merges them back onto the left beat's count.
        assert!(g.delete_beat_marker(2));
        let mut want = Grid::uniform(4, 4.0, 4);
        want.set_beat_subs(1, Some(3));
        want.set_beat_subs(3, Some(6));
        assert_eq!(g, want);

        // The other order does not round-trip, and cannot: two beats of different
        // sub-counts merge into one beat, which has room for one count. The right
        // one is what a delete spends, and a re-insert gives both halves the left's.
        let mut mixed = Grid::uniform(2, 2.0, 4);
        mixed.set_beat_subs(1, Some(3));
        assert!(mixed.delete_beat_marker(1));
        assert_eq!(mixed.subs(0), 4, "the surviving marker's beat keeps its count");
        assert_eq!(mixed.insert_beat_marker(1, 1.0), Some(1));
        assert_eq!(mixed.sub_override(1), None);
    }

    /// A refused insert must roll its shift back, overrides included — otherwise the
    /// rejection path is worse than the edit it declined.
    #[test]
    fn a_refused_insert_leaves_the_grid_untouched() {
        let mut g = Grid::uniform(3, 3.0, 4);
        g.set_beat_subs(0, Some(3));
        g.set_beat_subs(2, Some(6));
        g.set_beat_marker(1, 1.2);
        g.set_swing(Swing::mpc(0.4));
        let before = g;
        for (i, pos) in [(0, 0.5), (4, 2.5), (1, 0.0), (1, 3.0), (2, f64::NAN)] {
            assert_eq!(g.insert_beat_marker(i, pos), None, "i={i} pos={pos}");
            assert_eq!(g, before, "i={i} pos={pos}");
        }
    }

    /// The length cap is what makes the [`MIN_SLOT`] clamp mean something rather than
    /// merely say something: past `MIN_SLOT · 2⁵³` beats, `m ± MIN_SLOT` rounds back
    /// to `m`, so a drag clamps a marker exactly onto its neighbour and leaves the
    /// zero-width slot every other argument here assumes away.
    ///
    /// The narrowest slot a lane can reach is `MIN_SLOT / MAX_SUBS`, and the cap has
    /// to sit below the point where an ulp of a marker overtakes an `f32` ulp of
    /// *that* — the tighter of the two bounds, and the one 0349's exactness claim
    /// rests on. Checked here as arithmetic rather than as a remembered number.
    #[test]
    fn the_length_cap_keeps_the_min_slot_clamp_meaningful() {
        // Both bounds, as arithmetic rather than as a remembered number. The tighter
        // one is also a `const` assertion beside the constant, so raising the cap past
        // it is a compile error and not a test failure.
        const { assert!(MAX_LEN_BEATS * f64::EPSILON < MIN_SLOT) };
        const {
            assert!(MAX_LEN_BEATS * f64::EPSILON < (MIN_SLOT / MAX_SUBS as f64) * f32::EPSILON as f64)
        };

        let mut g = Grid::uniform(4, 1e300, 4);
        assert_eq!(g.len_beats(), MAX_LEN_BEATS);
        assert_eq!(g.set_len_beats(f64::MAX), MAX_LEN_BEATS);
        g.set_n_beats(MAX_BEATS);
        assert_eq!(g.len_beats(), MAX_LEN_BEATS);
        assert_increasing(&g);

        // At the ceiling every marker still separates from its neighbours, which is
        // the property the clamp is asserting. Dragged hard against both bounds.
        for i in 1..g.n_beats() {
            g.set_beat_marker(i, f64::MAX);
        }
        assert_increasing(&g);
        for i in (1..g.n_beats()).rev() {
            g.set_beat_marker(i, f64::MIN);
        }
        assert_increasing(&g);
        for i in 0..=g.n_beats() {
            let m = g.beat_marker(i);
            assert!(m + MIN_SLOT > m && m - MIN_SLOT < m, "MIN_SLOT vanishes at {m}");
        }
        // And an insert at the ceiling still splits rather than collapsing.
        let mut g = Grid::uniform(2, MAX_LEN_BEATS, 4);
        assert_eq!(g.insert_beat_marker(2, MAX_LEN_BEATS * 0.75), Some(2));
        assert_increasing(&g);
    }

    /// AC (the NaN one): randomised insert / delete / drag over a randomised grid
    /// never leaves a marker non-finite, out of order, or a slot degenerate.
    #[test]
    fn random_marker_edits_never_degenerate_the_grid() {
        let mut rng = Rng(0x0349_0001);
        for trial in 0..300 {
            let mut g = Grid::uniform(1 + trial % MAX_BEATS, 4.0, 1 + trial as u32 % MAX_SUBS);
            g.set_swing(Swing::mpc(rng.bipolar()).with_period(PERIODS[trial % PERIODS.len()]));
            for _ in 0..40 {
                let i = rng.below(MAX_BEATS as u32 + 2) as usize;
                // Wild positions on purpose: outside the pattern, and non-finite.
                let pos = match rng.below(8) {
                    0 => f64::NAN,
                    1 => f64::INFINITY,
                    2 => rng.bipolar() * 50.0,
                    _ => rng.unit() * g.len_beats(),
                };
                match rng.below(4) {
                    0 => {
                        g.set_beat_marker(i, pos);
                    }
                    1 => {
                        g.insert_beat_marker(i, pos);
                    }
                    2 => {
                        g.delete_beat_marker(i);
                    }
                    _ => g.set_beat_subs(i, Some(1 + rng.below(MAX_SUBS))),
                }
                assert!((1..=MAX_BEATS).contains(&g.n_beats()));
                assert_eq!(g.beat_marker(0), 0.0, "the start marker moved");
                assert_eq!(g.len_beats(), 4.0, "the end marker moved");
                for b in 0..g.n_beats() {
                    let (lo, hi) = (g.beat_marker(b), g.beat_marker(b + 1));
                    assert!(lo.is_finite() && hi.is_finite(), "non-finite marker");
                    assert!(hi - lo > 0.0, "degenerate beat slot {b}: {lo}..{hi}");
                    // And no subdivision slot inside it collapsed either — that span
                    // is what 0348's in-slot fraction divides by.
                    for k in 0..g.subs(b) {
                        let span = g.sub_pos(b, k + 1) - g.sub_pos(b, k);
                        assert!(span > 0.0, "degenerate sub slot {b}/{k}: {span}");
                        assert!(!g.locate(g.sub_pos(b, k)).frac.is_nan());
                    }
                }
            }
        }
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

    /// Every period worth exercising: the two named ones, a no-op period, periods that
    /// divide the common sub-counts and periods that deliberately do not.
    const PERIODS: [SwingPeriod; 8] = [
        SwingPeriod::Beat,
        SwingPeriod::Pair,
        SwingPeriod::Custom(1),
        SwingPeriod::Custom(3),
        SwingPeriod::Custom(4),
        SwingPeriod::Custom(5),
        SwingPeriod::Custom(8),
        SwingPeriod::Custom(MAX_SUBS as u8),
    ];

    /// Sub-to-sub gaps inside beat `b`. Every caller builds unit-width beats, so these
    /// are the beat fractions directly and no scaling rounding creeps in.
    fn beat_gaps(g: &Grid, b: usize) -> [f64; MAX_SUBS as usize] {
        let mut out = [f64::NAN; MAX_SUBS as usize];
        for k in 0..g.subs(b) {
            out[k as usize] = g.sub_pos(b, k + 1) - g.sub_pos(b, k);
        }
        out
    }

    /// AC: at `n = 4` and full swing the beat is **long-short-long-short**, exactly.
    ///
    /// The 0347 beat-wide warp gave `0.375, 0.375, 0.125, 0.125` here — subs 1 and 3
    /// were already where shuffle wants them and it was sub 2, the on-beat 8th, that
    /// got dragged to 0.75. It holds at 0.5 now.
    #[test]
    fn full_swing_at_sixteenths_is_long_short_long_short() {
        let mut g = Grid::uniform(4, 4.0, 4);
        g.set_swing(Swing::mpc(1.0));
        for b in 0..g.n_beats() {
            let base = b as f64;
            assert_eq!(g.sub_pos(b, 0), base);
            assert_eq!(g.sub_pos(b, 1), base + 0.375);
            assert_eq!(g.sub_pos(b, 2), base + 0.5, "the on-beat 8th must not move");
            assert_eq!(g.sub_pos(b, 3), base + 0.875);
            assert_eq!(g.sub_pos(b, 4), base + 1.0);
            assert_eq!(beat_gaps(&g, b)[..4], [0.375, 0.125, 0.375, 0.125], "b={b}");
        }
        // Negative amount mirrors it: short-long-short-long, same magnitudes.
        g.set_swing(Swing::mpc(-1.0));
        for b in 0..g.n_beats() {
            assert_eq!(beat_gaps(&g, b)[..4], [0.125, 0.375, 0.125, 0.375], "b={b}");
        }
    }

    /// AC: `n = 2` is untouched, bit-for-bit — the pair *is* the beat there, which is
    /// exactly why the beat-wide warp looked right. Checked against the pre-0365
    /// expression verbatim, on skewed markers so `hi - lo` is not 1.
    #[test]
    fn eighth_swing_is_bit_for_bit_what_it_shipped() {
        let mut rng = Rng(0x0365_0002);
        for i in 0..64 {
            let amount = -1.25 + 2.5 * (i as f64 / 63.0);
            let sw = Swing::mpc(amount);
            let mut g = Grid::uniform(4, 4.0, 2);
            g.set_swing(sw);
            g.set_beat_marker(1, 0.3 + rng.unit());
            g.set_beat_marker(2, 1.6 + rng.unit());
            g.set_beat_marker(3, 2.9 + rng.unit());
            for b in 0..g.n_beats() {
                let (lo, hi) = (g.beat_marker(b), g.beat_marker(b + 1));
                assert_eq!(g.sub_pos(b, 1), lo + sw.w(0.5) * (hi - lo), "a={amount} b={b}");
            }
        }
    }

    /// AC: at `n = 8` full swing is four long-short pairs, not two halves.
    #[test]
    fn full_swing_at_thirty_seconds_is_four_pairs() {
        let mut g = Grid::uniform(4, 4.0, 8);
        g.set_swing(Swing::mpc(1.0));
        let want = [0.1875, 0.0625, 0.1875, 0.0625, 0.1875, 0.0625, 0.1875, 0.0625];
        for b in 0..g.n_beats() {
            assert_eq!(beat_gaps(&g, b)[..8], want, "b={b}");
        }
    }

    /// The beat-wide warp is still reachable, and is what it always was: swing one
    /// subdivision level up — 8th-note swing on a 16ths lane.
    ///
    /// Bit-exact against the pre-0365 expression at dyadic `n`, where `n · w(k/n) / n`
    /// round-trips. At non-dyadic `n` it does not, so the standard there is the one
    /// `non_dyadic_sub_counts_match_the_old_grid_to_one_ulp` already holds the grid to.
    #[test]
    fn the_beat_period_reproduces_the_pre_ticket_warp() {
        let sw = Swing::mpc(1.0).with_period(SwingPeriod::Beat);
        for n in [2u32, 4, 8, 16] {
            let mut g = Grid::uniform(4, 4.0, n);
            g.set_swing(sw);
            for b in 0..g.n_beats() {
                for k in 1..n {
                    let want = b as f64 + sw.w(k as f64 / n as f64);
                    assert_eq!(g.sub_pos(b, k), want, "n={n} b={b} k={k}");
                }
            }
        }
        for n in [3u32, 6, 12] {
            let mut g = Grid::uniform(4, 4.0, n);
            g.set_swing(sw);
            for b in 0..g.n_beats() {
                for k in 1..n {
                    let want = b as f64 + sw.w(k as f64 / n as f64);
                    let got = g.sub_pos(b, k);
                    assert!(
                        (got - want).abs() <= 4.0 * f64::EPSILON * want.max(1.0),
                        "n={n} b={b} k={k}: {got} vs {want}"
                    );
                }
            }
        }
        // Which at n = 4 is the back-loaded beat 0347 shipped, kept as a setting.
        let mut g = Grid::uniform(4, 4.0, 4);
        g.set_swing(sw);
        assert_eq!(beat_gaps(&g, 0)[..4], [0.375, 0.375, 0.125, 0.125]);
    }

    /// AC: the odd-`n` rule. A period that does not divide the beat leaves a short
    /// trailing group warped across its own width — at `n = 3` that group holds one
    /// subdivision, which `w(0) = 0` leaves unswung on its own boundary while the pair
    /// before it shuffles normally. At `n = 6` the pairs come out even, so there is no
    /// leftover and three clean long-short pairs.
    #[test]
    fn a_short_trailing_period_is_warped_across_its_own_width() {
        let mut three = Grid::uniform(4, 4.0, 3);
        three.set_swing(Swing::mpc(1.0));
        for b in 0..three.n_beats() {
            let base = b as f64;
            assert_eq!(three.sub_pos(b, 0), base);
            assert_eq!(three.sub_pos(b, 1), base + 1.5 / 3.0, "the pair that exists swings");
            assert_eq!(three.sub_pos(b, 2), base + 2.0 / 3.0, "the leftover does not");
            assert_eq!(three.sub_pos(b, 3), base + 1.0);
        }
        // The whole-beat feel is still available on a triplet lane — it is a setting.
        three.set_swing(Swing::mpc(1.0).with_period(SwingPeriod::Beat));
        for b in 0..three.n_beats() {
            assert!(three.sub_pos(b, 2) > b as f64 + 0.8, "beat-wide pulls the third late");
        }

        let mut six = Grid::uniform(4, 4.0, 6);
        six.set_swing(Swing::mpc(1.0));
        for b in 0..six.n_beats() {
            let gaps = beat_gaps(&six, b);
            for p in 0..3 {
                assert!((gaps[2 * p] - 0.25).abs() < 1e-12, "b={b} p={p} {gaps:?}");
                assert!((gaps[2 * p + 1] - 1.0 / 12.0).abs() < 1e-12, "b={b} p={p} {gaps:?}");
            }
        }
    }

    /// AC: sub markers stay strictly increasing at every swing amount for every `n`,
    /// now swept across every period too — and over the **whole** sub-count range
    /// rather than [`SUB_COUNTS`], because the shapes at risk are the ones that leave a
    /// short trailing group (`n = 16` with `Custom(5)`, `n = 11` with `Custom(3)`) and
    /// those are exactly the counts the shorter list skips.
    #[test]
    fn subs_strictly_increase_at_every_swing_period() {
        let mut rng = Rng(0x0365_0347);
        for period in PERIODS {
            for n in 1..=MAX_SUBS {
                for i in 0..24 {
                    let amount = -1.25 + 2.5 * (i as f64 / 23.0);
                    let mut g = Grid::uniform(4, 4.0, n);
                    g.set_swing(Swing::mpc(amount).with_period(period));
                    g.set_beat_marker(1, 0.3 + rng.unit());
                    g.set_beat_marker(2, 1.6 + rng.unit());
                    g.set_beat_marker(3, 2.9 + rng.unit());
                    for b in 0..g.n_beats() {
                        let mut prev = g.sub_pos(b, 0);
                        assert_eq!(prev, g.beat_marker(b));
                        for k in 1..=n {
                            let p = g.sub_pos(b, k);
                            assert!(p > prev, "period={period:?} n={n} a={amount} b={b} k={k}: {prev} -> {p}");
                            prev = p;
                        }
                        assert_eq!(prev, g.beat_marker(b + 1));
                    }
                }
            }
        }
    }

    /// AC: `sub_pos(b, 0)` is `m[b]` and `sub_pos(b, k >= n)` is `m[b+1]`, **exactly**,
    /// at every swing amount and every period. `locate` scans these values to pick a
    /// beat, so one ULP of drift lands a hit in the wrong one.
    #[test]
    fn the_beat_markers_are_exact_at_every_amount_and_period() {
        let mut rng = Rng(0x0365_0E0E);
        for period in PERIODS {
            for shape in [SwingShape::Straight, SwingShape::Mpc] {
                for n in 1..=MAX_SUBS {
                    for _ in 0..16 {
                        let mut g = Grid::uniform(4, 4.0, n);
                        g.set_swing(Swing { shape, amount: rng.bipolar() * 1.5, period });
                        g.set_beat_marker(1, 0.3 + rng.unit());
                        g.set_beat_marker(2, 1.6 + rng.unit());
                        g.set_beat_marker(3, 2.9 + rng.unit());
                        for b in 0..g.n_beats() {
                            assert_eq!(g.sub_pos(b, 0), g.beat_marker(b), "period={period:?} n={n} b={b}");
                            for k in n..n + 4 {
                                assert_eq!(g.sub_pos(b, k), g.beat_marker(b + 1), "period={period:?} n={n} b={b} k={k}");
                            }
                        }
                    }
                }
            }
        }
    }

    /// [`Grid::sub_pos`] divides the warped offset out by `width` and multiplies it
    /// straight back in, so zero-swing bit-exactness rests on that being lossless.
    ///
    /// It is, for every width a lane can reach — but by exhaustion, not by an argument
    /// that scales: the first width where it fails is 22 (`22 · (15/22) = 14.999…`).
    /// [`MAX_SUBS`] is what keeps it true, so this is the test that fails if `MAX_SUBS`
    /// is ever raised past 21, rather than a silent ULP of drift in a straight lane.
    #[test]
    fn a_width_never_loses_the_subdivision_it_divides_out() {
        for width in 1..=MAX_SUBS {
            for m in 0..width {
                let round_tripped = width as f64 * (m as f64 / width as f64);
                assert_eq!(round_tripped, m as f64, "width={width} m={m}");
            }
        }
    }

    /// AC: zero swing reproduces the uniform grid at **every** period, to `f64`
    /// equality for dyadic `n`. The period does not enter — the width divides out
    /// exactly (above), so the identity warp does not care how the beat is tiled.
    #[test]
    fn zero_swing_reproduces_the_uniform_grid_at_every_period() {
        for period in PERIODS {
            for n in [1u32, 2, 4, 8, 16] {
                let step_beats = 1.0 / n as f64;
                let mut g = Grid::uniform(MAX_BEATS, MAX_BEATS as f64, n);
                g.set_swing(Swing::mpc(0.0).with_period(period));
                for b in 0..g.n_beats() {
                    for k in 0..n {
                        let i = (b as u32 * n + k) as f64;
                        assert_eq!(g.sub_pos(b, k), i * step_beats, "period={period:?} n={n} b={b} k={k}");
                    }
                }
            }
        }
    }

    /// And to a ULP or so for the non-dyadic combinations, which is the same standard
    /// `non_dyadic_sub_counts_match_the_old_grid_to_one_ulp` holds the shipped grid to.
    #[test]
    fn zero_swing_matches_the_uniform_grid_at_every_period() {
        for period in PERIODS {
            for n in [1u32, 2, 3, 4, 6, 8, 12, 16] {
                let step_beats = 1.0 / n as f64;
                let mut g = Grid::uniform(MAX_BEATS, MAX_BEATS as f64, n);
                g.set_swing(Swing::mpc(0.0).with_period(period));
                for b in 0..g.n_beats() {
                    for k in 0..n {
                        let want = (b as u32 * n + k) as f64 * step_beats;
                        let got = g.sub_pos(b, k);
                        assert!(
                            (got - want).abs() <= 4.0 * f64::EPSILON * want.max(1.0),
                            "period={period:?} n={n} b={b} k={k}: {got} vs {want}"
                        );
                    }
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
                    g.set_swing(Swing { shape, amount: rng.bipolar(), ..Swing::default() });
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

    /// `pos_of` and `locate` scan `sub_pos` itself rather than inverting the warp, so
    /// they needed no change for 0365 — this is the check that they did not.
    #[test]
    fn mapping_round_trips_at_every_swing_period() {
        let mut rng = Rng(0x0365_BEEF);
        for period in PERIODS {
            for n in SUB_COUNTS {
                for trial in 0..8 {
                    let mut g = Grid::uniform(4, 4.0, n);
                    g.set_swing(Swing::mpc(rng.bipolar()).with_period(period));
                    g.set_beat_marker(1, 0.4 + rng.unit() * 0.8);
                    g.set_beat_marker(2, 1.6 + rng.unit() * 0.8);
                    g.set_beat_marker(3, 2.7 + rng.unit() * 0.8);
                    g.set_beat_subs(trial % 4, Some(3)); // a tuplet in the mix
                    for _ in 0..200 {
                        let t = rng.unit() * g.len_beats();
                        let at = g.locate(t);
                        assert!(at.sub < g.subs(at.beat));
                        let back = g.pos_of(at);
                        assert!((back - t).abs() < 1e-12, "period={period:?} n={n} t={t} -> {at:?} -> {back}");
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

// ── Global subdivision indexing (ticket 0348) ─────────────────────────────────
//
// Appended as its own block so it sits clear of the geometry above: 0348 needs a
// single integer naming every subdivision marker in the pattern, plus the
// loop-extended form of it, and neither is a property of the warp.
//
// 0348 stores a hit as `(beat, sub, f, nudge)` while 0346's scheduler walks a
// *continuous looping* timeline. `total_subs` counts the slots and `sub_pos`
// places one; what was missing is the `index ↔ (beat, sub)` mapping between them,
// and the extension of it past one pass — a lane's p-lock cursor counts crossed
// subdivision slots (ADR 0007 §9), and that count runs on through the loop wrap.

impl Grid {
    /// `(beat, sub)` of subdivision index `i` within one pass of the pattern.
    /// Out-of-range indices clamp to the last slot rather than panicking on the
    /// audio thread, matching every other query here.
    pub fn sub_of_index(&self, index: u32) -> (usize, u32) {
        let mut rem = index;
        for b in 0..self.n_beats() {
            let n = self.subs(b);
            if rem < n {
                return (b, rem);
            }
            rem -= n;
        }
        let last = self.n_beats() - 1;
        (last, self.subs(last) - 1)
    }

    /// Subdivision index of `(beat, sub)` within one pass — the inverse of
    /// [`Grid::sub_of_index`]. Clamps its arguments the way [`Grid::sub_pos`] does.
    pub fn sub_index(&self, beat: usize, sub: u32) -> u32 {
        let b = beat.min(self.n_beats() - 1);
        let mut i = 0;
        for x in 0..b {
            i += self.subs(x);
        }
        i + sub.min(self.subs(b) - 1)
    }

    /// Position in beats of **global** slot `g`: `g` runs past `total_subs()` into
    /// the next pass of the pattern and negative into the previous one, so the
    /// scheduler can count straight through the loop wrap.
    ///
    /// `slot_pos(total_subs())` is the pattern end exactly — the pass offset is a
    /// whole multiple of `len_beats()` and `sub_pos(0, 0)` is a pinned marker.
    pub fn slot_pos(&self, g: i64) -> f64 {
        let total = self.total_subs() as i64; // >= 1: n_beats >= 1 and subs >= 1
        let pass = g.div_euclid(total);
        let (b, k) = self.sub_of_index(g.rem_euclid(total) as u32);
        pass as f64 * self.len_beats() + self.sub_pos(b, k)
    }

    /// Width in beats of global slot `g` — what 0348's in-slot fraction is a
    /// fraction *of*, and what its nudge clamp is measured against.
    #[inline]
    pub fn slot_span(&self, g: i64) -> f64 {
        self.slot_pos(g.saturating_add(1)) - self.slot_pos(g)
    }

    /// The global slot owning position `t`: the largest `g` with
    /// `slot_pos(g) <= t`. Non-finite input answers `0`.
    pub fn slot_at(&self, t: f64) -> i64 {
        if !t.is_finite() {
            return 0;
        }
        let len = self.len_beats();
        // A float→int cast saturates, so a large `t` would otherwise reach the
        // multiply below as `i64::MAX` and overflow it — a debug panic on the
        // audio thread. `MAX_PASS` is far beyond any beat position a host can
        // report and leaves the product nowhere near the `i64` edge.
        const MAX_PASS: f64 = 1e15;
        let pass = (t / len).floor().clamp(-MAX_PASS, MAX_PASS);
        if !pass.is_finite() {
            return 0;
        }
        let at = self.locate(t - pass * len);
        pass as i64 * self.total_subs() as i64 + self.sub_index(at.beat, at.sub) as i64
    }
}

#[cfg(test)]
mod index_tests {
    use super::*;

    #[test]
    fn index_round_trips_across_a_tuplet_beat() {
        let mut g = Grid::uniform(4, 4.0, 4);
        g.set_beat_subs(2, Some(3));
        assert_eq!(g.total_subs(), 15);
        for i in 0..g.total_subs() {
            let (b, k) = g.sub_of_index(i);
            assert!(k < g.subs(b), "index {i} → ({b}, {k})");
            assert_eq!(g.sub_index(b, k), i);
            assert_eq!(g.slot_pos(i as i64), g.sub_pos(b, k));
        }
        // Past the end clamps to the last slot, and `sub_index` clamps its
        // arguments the same way `sub_pos` does.
        assert_eq!(g.sub_of_index(999), (3, 3));
        assert_eq!(g.sub_index(99, 99), 14);
    }

    #[test]
    fn global_slots_run_through_the_loop_wrap() {
        let g = Grid::uniform(4, 4.0, 4);
        assert_eq!(g.slot_pos(0), 0.0);
        assert_eq!(g.slot_pos(16), 4.0, "one whole pass on");
        assert_eq!(g.slot_pos(17), 4.25);
        assert_eq!(g.slot_pos(-1), -0.25);
        assert_eq!(g.slot_pos(-16), -4.0);
        for i in -20..40 {
            assert_eq!(g.slot_span(i), 0.25, "slot {i}");
        }
    }

    #[test]
    fn slot_at_locates_across_passes_and_uneven_beats() {
        let mut g = Grid::uniform(4, 4.0, 4);
        g.set_beat_subs(1, Some(2)); // 4 + 2 + 4 + 4 = 14 slots
        assert_eq!(g.total_subs(), 14);
        for i in -14..28 {
            let p = g.slot_pos(i);
            assert_eq!(g.slot_at(p), i, "slot {i} at {p}");
            // Anywhere inside the slot resolves to the same slot.
            assert_eq!(g.slot_at(p + g.slot_span(i) * 0.5), i, "mid-slot {i}");
        }
        assert_eq!(g.slot_at(f64::NAN), 0);
        assert_eq!(g.slot_at(f64::INFINITY), 0);
    }
}
