//! The lane data model (ADR 0007 §4, ticket 0348).
//!
//! A pattern is **one track's** independent lane: the timing geometry of a
//! [`Grid`] — beat markers, per-beat subdivision counts, the swing warp — plus a
//! fixed-capacity list of **hits** positioned against it. Position is a
//! coordinate, not an array index:
//!
//! ```text
//! t = sub_pos(b, k) + f · (sub_pos(b, k+1) - sub_pos(b, k)) + nudge
//! ```
//!
//! The two-part offset is the point (ADR 0007 §4). `f ∈ [0, 1)` is a fraction of
//! the slot, so it **scales** with it — and `f = 0` welds a hit to its
//! subdivision marker, exactly, so a snapped pattern survives every groove edit
//! with no re-quantise pass. `nudge` is absolute (in [`TICKS_PER_BEAT`] ticks) and
//! does *not* scale, so a deliberate flam survives a swing change unchanged.
//!
//! Grids are **per-lane**, which is what preserves the ADR 0001 §2 polymeter the
//! old `len`/`step_beats` pair provided: lanes of different beat counts phase.
//!
//! **The list is kept in fire order.** Every mutation that can move a hit —
//! insert, an `f`/`nudge` edit, any grid edit — re-sorts, and the ordering key is
//! the resolved fire time itself. That is what lets [`crate::lane`] walk hits with
//! a single monotonic cursor instead of scanning the lane per block, and it is the
//! invariant the monotonicity property test pins.
//!
//! Trig **attributes** (probability, retrig n/m/curve/velocity ramp) live on the
//! hit — they have no base to revert to. Continuous params that *do* have a base
//! are p-locked, and a lock now belongs to a **hit** rather than to a grid cell
//! (ADR 0001 §3a's split is not revisited). `rgb` — the hit's colour — *is* its macro
//! vector, consumed at trig time by [`crate::lane`] (0351); `y` is stored and not yet
//! consumed (0350).

use crate::grid::{Grid, GridPos, MIN_SLOT};

/// A 16th note in quarter-note beats.
pub const SIXTEENTH: f64 = 0.25;
/// A straight 8th note in beats.
pub const EIGHTH: f64 = 0.5;
/// An 8th-note triplet in beats (3 per beat → triplet feel).
pub const EIGHTH_TRIPLET: f64 = 1.0 / 3.0;

/// Maximum hits in one lane. Storage is a fixed-capacity array and an
/// over-capacity insert **drops** rather than allocating (ADR 0007 §9), matching
/// the same policy on the audio thread's hit buffer.
///
/// Four times the 16 snap targets of the default grid, and exactly a 16-beat lane
/// of quarter-note subdivisions. A lane subdivided finer than that can hold fewer
/// hits than it has slots; that is the deliberate ceiling, not an oversight.
pub const MAX_HITS: usize = 64;

/// Ticks per quarter-note beat — the unit of [`Hit::nudge`].
///
/// Sized so the ±½ [`MIN_SLOT`] clamp is an exact integer ([`MAX_NUDGE_TICKS`])
/// and one tick (~1 µs at 120 bpm) is finer than a sample at any sane rate, so a
/// nudge round-trips through the UI without drifting.
pub const TICKS_PER_BEAT: f64 = 30_720.0;

/// The `nudge` clamp in ticks: ±½ [`MIN_SLOT`] (ADR 0007 §9). Every write path
/// clamps to it.
///
/// This is the **storage** bound. It is not on its own sufficient to keep a hit
/// from reordering past its neighbours, because a subdivision slot can be far
/// narrower than a beat slot (`MIN_SLOT / MAX_SUBS`); [`Pattern::fire_beat`]
/// therefore also clamps the resolved nudge to ±½ of the hit's *own* slot, which
/// is the bound [`crate::lane`]'s lookahead window is sized from.
pub const MAX_NUDGE_TICKS: i16 = 240;
const _: () = assert!(MAX_NUDGE_TICKS as f64 == TICKS_PER_BEAT * MIN_SLOT * 0.5);

/// Retrig timing curve — how the `n` hits are spaced across the window.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum RetrigCurve {
    /// Evenly spaced.
    #[default]
    Even,
    /// Speeding up — gaps shrink over the window (a roll into the next hit).
    Accel,
    /// Slowing down — gaps grow over the window.
    Decel,
}

impl RetrigCurve {
    /// Map a normalised index `u = j/n ∈ [0, 1)` to a normalised position in the
    /// window `[0, 1)`. `pos(0) = 0` always (first hit at the window start).
    #[inline]
    pub fn position(self, u: f64) -> f64 {
        match self {
            RetrigCurve::Even => u,
            RetrigCurve::Accel => u.sqrt(), // gaps shrink
            RetrigCurve::Decel => u * u,    // gaps grow
        }
    }
}

/// Retrig macro on a trig: fire `n` hits across `m` subdivision slots (ADR 0001 §2).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Retrig {
    /// Hit count. `1` (or `0`) = no retrig, a single hit.
    pub n: u8,
    /// Window span in **subdivision slots**, measured from the slot the hit sits in.
    pub m: u8,
    /// Timing curve across the window.
    pub curve: RetrigCurve,
    /// Velocity at the last hit (the first uses the hit's velocity); a ramp.
    pub vel_end: f32,
}

impl Default for Retrig {
    fn default() -> Self {
        Self {
            n: 1,
            m: 1,
            curve: RetrigCurve::Even,
            vel_end: 1.0,
        }
    }
}

impl Retrig {
    #[inline]
    pub fn is_retrig(&self) -> bool {
        self.n >= 2 && self.m >= 1
    }
}

/// A continuous track/engine parameter a p-lock can override (ADR 0001 §3a).
/// Trig attributes (probability, retrig, velocity) are *not* here — they live on
/// the trig (0048). The send amount joins once 0051 lands.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LockParam {
    Gain,
    Pan,
    Decay,
    Tone,
    Pitch,
    /// Delay send amount — p-locking this high on a hit is the dub throw (0051).
    Send,
}

/// Number of lockable params; the lock table and resolver are sized to it.
pub const N_LOCK_PARAMS: usize = 6;

impl LockParam {
    #[inline]
    pub fn index(self) -> usize {
        match self {
            LockParam::Gain => 0,
            LockParam::Pan => 1,
            LockParam::Decay => 2,
            LockParam::Tone => 3,
            LockParam::Pitch => 4,
            LockParam::Send => 5,
        }
    }
}

/// How a p-lock ends (the step-shape subset — no ramp yet).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Termination {
    /// Hold for `n` **subdivision slots** crossed on this lane, then release back
    /// to base. `n = 1` is the momentary spike.
    ///
    /// Slots, not a duration: with per-beat sub-counts (ADR 0007 §2) a slot's
    /// width varies down the lane, so a revert hold is a count of grid positions.
    /// That is the semantics [`crate::lane::LaneState`] has always had — it is
    /// stated here because the grid is no longer uniform, so "lane tick" no longer
    /// names one thing (ADR 0007 §9).
    Revert { n: u16 },
    /// Hold until the next lock on this param; persists across the loop wrap.
    Latch,
}

/// A per-hit parameter lock (step shape).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Lock {
    pub value: f32,
    pub termination: Termination,
}

/// One freely-positioned hit: where it sits against the grid, its trig
/// attributes, its modulation coordinates, and its p-locks.
///
/// The fields are public data, but `f` and `nudge` are only ever *canonicalised*
/// on their way into a [`Pattern`] — construct a hit however you like, then hand
/// it to [`Pattern::insert`], which clamps it and places it in fire order.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Hit {
    /// Beat slot this hit hangs off.
    pub beat: u16,
    /// Subdivision marker within that beat.
    pub sub: u8,
    /// Fraction of the way to the next subdivision marker, `[0, 1)`. Proportional:
    /// it scales with the slot. `0` welds the hit to its marker.
    pub f: f32,
    /// Absolute offset in [`TICKS_PER_BEAT`] ticks, clamped to ±[`MAX_NUDGE_TICKS`].
    /// **Unscaled** by swing — a flam is an absolute quantity.
    pub nudge: i16,
    /// The lane's modulation axis (ADR 0007 §6). Stored, not yet consumed (0350).
    pub y: f32,
    /// Per-hit macro vector — the hit's **colour** (ADR 0007 §7). Normalised
    /// `0.00–1.00` per channel; drives the track's three macro slots at trig time
    /// (0351), outranking a p-lock on the same slot.
    ///
    /// A channel of [`crate::flavour::NO_COLOUR`] means the hit carries no colour, and
    /// its slots fall through to the p-lock/base. Black is *not* that: `[0, 0, 0]` is a
    /// colour, and it sends zero to all three slots.
    pub rgb: [f32; 3],
    /// Equal-tempered MIDI note (fractional allowed).
    pub note: f32,
    /// 0..1.
    pub velocity: f32,
    /// Fire probability per pass: `>= 1.0` always, `<= 0.0` never.
    pub probability: f32,
    pub retrig: Retrig,
    /// Sparse per-param lock table, indexed by [`LockParam::index`]. Held on the
    /// hit rather than in a side table so a lock cannot come adrift of the hit it
    /// belongs to when the list is re-sorted.
    pub locks: [Option<Lock>; N_LOCK_PARAMS],
}

impl Default for Hit {
    fn default() -> Self {
        Self {
            beat: 0,
            sub: 0,
            f: 0.0,
            nudge: 0,
            y: 0.5,
            // Uncoloured, not black: a fresh hit must leave the host macro params (and
            // any p-lock on them) in charge, and black is a real macro vector.
            rgb: [crate::flavour::NO_COLOUR; 3],
            note: 36.0, // C2
            velocity: 1.0,
            probability: 1.0,
            retrig: Retrig::default(),
            locks: [None; N_LOCK_PARAMS],
        }
    }
}

impl Hit {
    /// A default hit welded to subdivision `sub` of `beat` (`f = 0`, no nudge).
    pub fn at(beat: u16, sub: u8) -> Self {
        Self { beat, sub, ..Self::default() }
    }
}

/// One track's lane: its timing geometry plus its hits, in fire order.
///
/// The grid is private and mutated only through [`Pattern::edit_grid`]: moving a
/// marker or changing the swing re-times every hit, so the fire order the list is
/// stored in has to be re-established, and a `&mut Grid` handed out raw would
/// silently skip that.
#[derive(Copy, Clone, Debug)]
pub struct Pattern {
    grid: Grid,
    hits: [Hit; MAX_HITS],
    n_hits: usize,
}

impl Default for Pattern {
    fn default() -> Self {
        Self {
            grid: Grid::default(),
            hits: [Hit::default(); MAX_HITS],
            n_hits: 0,
        }
    }
}

impl Pattern {
    // ── geometry ──────────────────────────────────────────────────────────────

    #[inline]
    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// Edit the lane's geometry, then re-establish fire order. Every grid
    /// mutation must go through here — see the type's note.
    ///
    /// This is the **relative** door (ADR 0007 §5): the hits' `(beat, sub, f)` are
    /// left alone and the geometry moves underneath them, so they rubber-band with
    /// the slots they hang off. [`Pattern::edit_grid_preserving_times`] is the
    /// absolute one, and marker insert/delete is the only gesture that takes it.
    pub fn edit_grid(&mut self, f: impl FnOnce(&mut Grid)) {
        f(&mut self.grid);
        self.canonicalise();
    }

    /// Set the lane's beat count *and* its length in beats together, so `n` beats
    /// of one beat each. This is the polymeter control the old `len` was: lanes of
    /// different beat counts loop at different periods and phase.
    pub fn set_grid_beats(&mut self, n_beats: usize) {
        self.edit_grid(|g| {
            g.set_n_beats(n_beats);
            g.set_len_beats(g.n_beats() as f64);
        });
    }

    /// Set the lane-wide subdivision count — the old `step_beats`, expressed as
    /// geometry.
    pub fn set_grid_subs(&mut self, subs: u32) {
        self.edit_grid(|g| g.set_default_subs(subs));
    }

    /// Subdivision markers in one pass of this lane — the snap-target count, and
    /// the range a slot index runs over.
    #[inline]
    pub fn total_subs(&self) -> u32 {
        self.grid.total_subs()
    }

    // ── the hit list ──────────────────────────────────────────────────────────

    /// The hits, in fire order.
    #[inline]
    pub fn hits(&self) -> &[Hit] {
        &self.hits[..self.n_hits]
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.n_hits
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.n_hits == 0
    }

    /// Insert a hit, canonicalising its position and placing it in fire order.
    /// Returns its index, or `None` when the lane is at [`MAX_HITS`] — an
    /// over-capacity insert **drops**, it never allocates (ADR 0007 §9).
    pub fn insert(&mut self, hit: Hit) -> Option<usize> {
        if self.n_hits >= MAX_HITS {
            return None;
        }
        let h = self.canonical(hit);
        self.hits[self.n_hits] = h;
        self.n_hits += 1;
        // One insertion-sort step is enough: the rest of the list is already in
        // fire order. In place, so no allocation on the audio thread.
        let t = self.fire_beat(self.n_hits - 1);
        let mut j = self.n_hits - 1;
        while j > 0 && self.fire_beat(j - 1) > t {
            self.hits[j] = self.hits[j - 1];
            j -= 1;
        }
        self.hits[j] = h;
        Some(j)
    }

    /// Remove the hit at `index` (a no-op past the end), preserving fire order.
    pub fn remove(&mut self, index: usize) {
        if index >= self.n_hits {
            return;
        }
        for i in index..self.n_hits - 1 {
            self.hits[i] = self.hits[i + 1];
        }
        self.n_hits -= 1;
    }

    /// Drop every hit. The geometry is kept — clearing a lane is not a groove edit.
    pub fn clear_all(&mut self) {
        self.n_hits = 0;
    }

    /// Move a hit within its slot. Both parts are clamped ([`MAX_NUDGE_TICKS`],
    /// and `f` into `[0, 1)`) and the list is re-sorted, because this is exactly
    /// the edit that can change fire order.
    pub fn set_offset(&mut self, index: usize, f: f32, nudge: i16) {
        if index >= self.n_hits {
            return;
        }
        self.hits[index].f = f;
        self.hits[index].nudge = nudge;
        self.canonicalise();
    }

    /// The first hit sitting in subdivision slot `slot` of this lane, if any.
    ///
    /// A slot past the lane's end has no hit — [`Grid::sub_of_index`] clamps, but
    /// folding an out-of-range slot onto the last live one would let a stale
    /// editor cell edit a hit it is not pointing at.
    pub fn hit_index_at_slot(&self, slot: u32) -> Option<usize> {
        let (b, k) = self.slot_of(slot)?;
        self.hits[..self.n_hits]
            .iter()
            .position(|h| h.beat as usize == b && h.sub as u32 == k)
    }

    /// `(beat, sub)` of a live subdivision slot, or `None` past the lane's end.
    #[inline]
    fn slot_of(&self, slot: u32) -> Option<(usize, u32)> {
        (slot < self.grid.total_subs()).then(|| self.grid.sub_of_index(slot))
    }

    // ── p-locks, keyed on (hit, param) ────────────────────────────────────────

    /// Set a p-lock on a hit. Out-of-range hit indices are ignored.
    pub fn set_lock(&mut self, hit: usize, param: LockParam, lock: Lock) {
        if hit < self.n_hits {
            self.hits[hit].locks[param.index()] = Some(lock);
        }
    }

    /// Clear a p-lock on a hit.
    pub fn clear_lock(&mut self, hit: usize, param: LockParam) {
        if hit < self.n_hits {
            self.hits[hit].locks[param.index()] = None;
        }
    }

    /// Paint a hit's colour — its macro vector (ADR 0007 §7). Channels are stored as
    /// given (normalised `0.00–1.00`; [`crate::flavour::colour_override`] clamps on the
    /// way to the slots). Out-of-range hit indices are ignored.
    ///
    /// Not a position edit, so it cannot change fire order and does not re-sort.
    ///
    /// A hit's colour is snapshotted when the scheduler **resolves** it, which is up to
    /// half a slot ahead of its fire time (ADR 0007 §9), so a repaint can miss a hit
    /// already sitting in the lookahead window and land from the next pass. Same
    /// snapshot semantics `note` and `velocity` have had since 0346.
    pub fn set_colour(&mut self, hit: usize, rgb: [f32; 3]) {
        if hit < self.n_hits {
            self.hits[hit].rgb = rgb;
        }
    }

    /// Strip a hit's colour, handing its macro slots back to the p-lock/base.
    pub fn clear_colour(&mut self, hit: usize) {
        self.set_colour(hit, [crate::flavour::NO_COLOUR; 3]);
    }

    /// The lock (if any) on hit `hit` for `param_index`.
    #[inline]
    pub fn lock_at(&self, hit: usize, param_index: usize) -> Option<Lock> {
        self.hits()
            .get(hit)
            .and_then(|h| h.locks.get(param_index).copied().flatten())
    }

    // ── slot-keyed edits (the snapped-cell path the faceplate drives) ─────────

    /// Enable a hit welded to subdivision slot `slot` with note/velocity, adding
    /// one if the slot is empty. The snapped-editing verb: `f = 0`, no nudge.
    /// A slot past the lane's end is a no-op, as every verb below is.
    pub fn set(&mut self, slot: usize, note: f32, velocity: f32) {
        match self.hit_index_at_slot(slot as u32) {
            Some(i) => {
                self.hits[i].note = note;
                self.hits[i].velocity = velocity;
            }
            None => {
                if let Some((b, k)) = self.slot_of(slot as u32) {
                    self.insert(Hit {
                        note,
                        velocity,
                        ..Hit::at(b as u16, k as u8)
                    });
                }
            }
        }
    }

    /// Set the fire probability of the hit in `slot`, adding one if empty.
    pub fn set_probability(&mut self, slot: usize, probability: f32) {
        self.ensure_at_slot(slot);
        if let Some(i) = self.hit_index_at_slot(slot as u32) {
            self.hits[i].probability = probability;
        }
    }

    /// Set the retrig macro of the hit in `slot`, adding one if empty.
    pub fn set_retrig(&mut self, slot: usize, retrig: Retrig) {
        self.ensure_at_slot(slot);
        if let Some(i) = self.hit_index_at_slot(slot as u32) {
            self.hits[i].retrig = retrig;
        }
    }

    /// Remove the hit in `slot`, if there is one.
    pub fn clear(&mut self, slot: usize) {
        if let Some(i) = self.hit_index_at_slot(slot as u32) {
            self.remove(i);
        }
    }

    /// Slot-keyed toggle: remove the hit in `slot`, or add a default one there.
    pub fn toggle(&mut self, slot: usize) {
        match self.hit_index_at_slot(slot as u32) {
            Some(i) => self.remove(i),
            None => self.ensure_at_slot(slot),
        }
    }

    fn ensure_at_slot(&mut self, slot: usize) {
        if self.hit_index_at_slot(slot as u32).is_some() {
            return;
        }
        if let Some((b, k)) = self.slot_of(slot as u32) {
            self.insert(Hit::at(b as u16, k as u8));
        }
    }

    // ── resolution onto the timeline ──────────────────────────────────────────

    /// Resolved fire time of hit `index`, in beats from the pattern start.
    ///
    /// `f = 0` and no nudge returns [`Grid::sub_pos`] **exactly** — welding is an
    /// `f64` equality, not an epsilon, so a snapped pattern is bit-identical
    /// through any groove edit.
    ///
    /// The nudge is clamped a second time here, to ±½ of the hit's own slot. The
    /// [`MAX_NUDGE_TICKS`] storage clamp is in beats and a subdivision slot can be
    /// narrower than [`MIN_SLOT`]; this is the clamp that actually holds a hit
    /// inside the `[-½, +1½)`-slot displacement bound [`crate::lane`]'s window is
    /// sized from.
    pub fn fire_beat(&self, index: usize) -> f64 {
        if index >= self.n_hits {
            return 0.0;
        }
        let h = &self.hits[index];
        let b = (h.beat as usize).min(self.grid.n_beats() - 1);
        let k = (h.sub as u32).min(self.grid.subs(b) - 1);
        let p0 = self.grid.sub_pos(b, k);
        let span = self.grid.sub_pos(b, k + 1) - p0;
        let half = 0.5 * span;
        let nudge = (h.nudge as f64 / TICKS_PER_BEAT).clamp(-half, half);
        let f = h.f as f64;
        // The `f == 0` branch is explicit so welding does not depend on
        // `p0 + 0.0 * span` rounding — it is exact, but the guarantee is the point.
        let t = if f <= 0.0 { p0 + nudge } else { p0 + f * span + nudge };
        // The outer markers are the pattern bounds (ADR 0007 §2): a hit cannot
        // fire outside the pass it belongs to, which is also what keeps the global
        // fire order non-decreasing across the loop wrap.
        t.clamp(0.0, self.grid.len_beats())
    }

    /// Width in beats of the subdivision slot hit `index` sits in.
    pub fn slot_span(&self, index: usize) -> f64 {
        if index >= self.n_hits {
            return 0.0;
        }
        let h = &self.hits[index];
        let b = (h.beat as usize).min(self.grid.n_beats() - 1);
        let k = (h.sub as u32).min(self.grid.subs(b) - 1);
        self.grid.sub_pos(b, k + 1) - self.grid.sub_pos(b, k)
    }

    /// The hit at a **global** index — `pass · len() + i`, so the scheduler can
    /// count straight through the loop wrap. `None` on an empty lane.
    #[inline]
    pub fn hit_at(&self, global: i64) -> Option<Hit> {
        if self.n_hits == 0 {
            return None;
        }
        Some(self.hits[global.rem_euclid(self.n_hits as i64) as usize])
    }

    /// Fire time of a global hit index on the looping timeline.
    pub fn fire_beat_at(&self, global: i64) -> f64 {
        if self.n_hits == 0 {
            return 0.0;
        }
        let n = self.n_hits as i64;
        let pass = global.div_euclid(n) as f64;
        pass * self.grid.len_beats() + self.fire_beat(global.rem_euclid(n) as usize)
    }

    /// Slot width for a global hit index (the same for every pass).
    #[inline]
    pub fn slot_span_at(&self, global: i64) -> f64 {
        if self.n_hits == 0 {
            return 0.0;
        }
        self.slot_span(global.rem_euclid(self.n_hits as i64) as usize)
    }

    /// The span a retrig of `m` slots covers from the slot hit `global` sits in —
    /// real grid slots, so a retrig over a tuplet beat or a swung pair follows the
    /// geometry rather than a nominal step width.
    pub fn retrig_span(&self, global: i64, m: u8) -> f64 {
        if self.n_hits == 0 {
            return 0.0;
        }
        let n = self.n_hits as i64;
        let pass = global.div_euclid(n);
        let h = self.hits[global.rem_euclid(n) as usize];
        let b = (h.beat as usize).min(self.grid.n_beats() - 1);
        let k = (h.sub as u32).min(self.grid.subs(b) - 1);
        let g = pass
            .saturating_mul(self.grid.total_subs() as i64)
            .saturating_add(self.grid.sub_index(b, k) as i64);
        self.grid.slot_pos(g.saturating_add(m as i64)) - self.grid.slot_pos(g)
    }

    /// The global index of the first hit firing at or after `t`. On an empty lane
    /// this is `0` and nothing will resolve.
    pub fn hit_at_or_after(&self, t: f64) -> i64 {
        let n = self.n_hits as i64;
        if n == 0 || !t.is_finite() {
            return 0;
        }
        let len = self.grid.len_beats();
        // Clamped for the same reason as [`Grid::slot_at`]: a float→int cast
        // saturates, and `pass * n` on a saturated `pass` overflows.
        let pass = (t / len).floor().clamp(-1e15, 1e15);
        let local = t - pass * len;
        let pass = pass as i64;
        for i in 0..self.n_hits {
            if self.fire_beat(i) >= local {
                return pass * n + i as i64;
            }
        }
        (pass + 1) * n
    }

    // ── invariants ────────────────────────────────────────────────────────────

    /// Clamp every hit into the current geometry and re-establish fire order.
    /// Runs after any edit that can move a hit — including a grid edit, which
    /// moves all of them at once.
    fn canonicalise(&mut self) {
        for i in 0..self.n_hits {
            self.hits[i] = self.canonical(self.hits[i]);
        }
        // Keys first: the sort swaps hits, and recomputing the key mid-sort would
        // read a moved hit.
        let mut key = [0.0_f64; MAX_HITS];
        for (i, k) in key.iter_mut().enumerate().take(self.n_hits) {
            *k = self.fire_beat(i);
        }
        // Insertion sort: in place (so allocation-free on the audio thread, where
        // grid edits arrive as commands) and linear on the near-sorted list a
        // single edit leaves.
        for i in 1..self.n_hits {
            let (k, h) = (key[i], self.hits[i]);
            let mut j = i;
            while j > 0 && key[j - 1] > k {
                key[j] = key[j - 1];
                self.hits[j] = self.hits[j - 1];
                j -= 1;
            }
            key[j] = k;
            self.hits[j] = h;
        }
    }

    /// A hit clamped into the current geometry: a live `(beat, sub)`, `f ∈ [0, 1)`,
    /// `nudge` within ±[`MAX_NUDGE_TICKS`]. Non-finite `f` reads as `0` (welded)
    /// rather than poisoning every later comparison with NaN.
    ///
    /// Shrinking a lane therefore **re-snaps** the hits whose slot it removed onto
    /// the nearest surviving one, rather than dropping them or holding them
    /// inertly off the end. That is the relative-preserving default of ADR 0007 §5
    /// — the same rule a marker drag follows — and it can stack several hits on one
    /// slot, where the snapped editor only reaches the first. The absolute-time
    /// rule §5 gives *insert and delete* is ticket 0349's, and rebuilding
    /// `(beat, sub, f)` from a hit's current time is where that lands.
    fn canonical(&self, mut h: Hit) -> Hit {
        let b = (h.beat as usize).min(self.grid.n_beats() - 1);
        let k = (h.sub as u32).min(self.grid.subs(b) - 1);
        h.beat = b as u16;
        h.sub = k as u8;
        h.f = if h.f.is_finite() { h.f.clamp(0.0, F_MAX) } else { 0.0 };
        h.nudge = h.nudge.clamp(-MAX_NUDGE_TICKS, MAX_NUDGE_TICKS);
        h
    }

    // ── marker edits: drag is relative, insert/delete absolute (0349) ─────────
    //
    // ADR 0007 §5's two rules are opposite on purpose, and the asymmetry is in the
    // API rather than emergent from whichever path was written first. Both gestures
    // read the same way to a user, and neither is a special case of the other:
    //
    //   drag   — a pure clamped write to `m[i]`, no hit record touched. Slot `i-1`
    //            stretches and slot `i` squashes at once, so hits rubber-band in
    //            *both* directions from one grab, free from the fractional storage.
    //   insert — the slot splits and every hit stays where it is on screen, because
    //   delete   its `(beat, sub, f)` is rebuilt from the absolute time it had before
    //            the edit.

    /// Drag beat marker `i` to `pos`, **preserving relative position**: the hits in
    /// the two slots either side rubber-band with their slot, and not one hit record
    /// is written. Returns the position actually taken.
    ///
    /// That is what makes undo cheap — one stored `f64` restores every apparent hit
    /// position, however many hits appear to move. The clamp and the pinned outer
    /// markers are [`Grid::set_beat_marker`]'s, which is the only path into the
    /// marker array, so a drag can never produce a degenerate slot.
    ///
    /// The list is still re-sorted afterwards: `nudge` is absolute, so a hit near a
    /// slot that just narrowed can genuinely overtake its neighbour. Re-sorting
    /// permutes records; it does not change one.
    pub fn drag_beat_marker(&mut self, i: usize, pos: f64) -> f64 {
        let mut taken = self.grid.beat_marker(i);
        self.edit_grid(|g| taken = g.set_beat_marker(i, pos));
        taken
    }

    /// Insert a beat marker at `pos`, taking index `i` and splitting slot `i - 1` —
    /// **preserving absolute time**, so the split moves nothing on screen. Returns
    /// the index taken, or `None` if [`Grid::insert_beat_marker`] refused it.
    ///
    /// `pos` takes the same clamp a drag does, so it is not necessarily where the
    /// marker ends up — an undo record wants `grid().beat_marker(i)` afterwards, not
    /// the `pos` it asked for.
    pub fn insert_beat_marker(&mut self, i: usize, pos: f64) -> Option<usize> {
        let mut taken = None;
        self.edit_grid_preserving_times(|g| taken = g.insert_beat_marker(i, pos));
        taken
    }

    /// Delete beat marker `i`, merging slots `i - 1` and `i` — **preserving absolute
    /// time**, so the merge moves nothing on screen. Returns whether it was deleted.
    ///
    /// A hit welded to the deleted marker (`f = 0`) lands at a non-zero `f` in the
    /// merged slot, which is correct: what a delete preserves is where the hit *is*,
    /// not that it happened to be snapped.
    pub fn delete_beat_marker(&mut self, i: usize) -> bool {
        let mut deleted = false;
        self.edit_grid_preserving_times(|g| deleted = g.delete_beat_marker(i));
        deleted
    }

    /// The **absolute**-time door into the geometry, and the counterpart to
    /// [`Pattern::edit_grid`]'s relative one: resolve every hit's grid position,
    /// apply the edit, rebuild `(beat, sub, f)` from those positions.
    ///
    /// The **nudge is deliberately held out of the sandwich**. It is an absolute tick
    /// offset that by definition does not follow the geometry (ADR 0007 §4), so the
    /// stored ticks ride through untouched and what is resolved and rebuilt is the
    /// grid-relative part.
    ///
    /// But *resolving* a nudge is not geometry-free: [`Pattern::fire_beat`] clamps it
    /// to ±½ of the hit's own slot (0348), so a slot that changes width changes how
    /// much of a large nudge survives — by up to ½ [`MIN_SLOT`], which is 3.9 ms at
    /// 120 bpm and nothing like an ulp. Rebuilding the grid part alone would preserve
    /// where a hit *hangs* and still move where it *fires*. So each hit gets two
    /// candidate placements — the grid part as it stood, and the same shifted by the
    /// change in the resolved nudge — and takes whichever resolves closer to the fire
    /// time being preserved. When nothing changed the first is exact and wins by
    /// construction, so the common path is still one `locate` and no arithmetic.
    ///
    /// **The shifted candidate is not always available**, and this is the one place
    /// the absolute rule is bounded rather than exact: a hit at the very start of the
    /// pattern whose slot has just widened would need a *negative* grid position to
    /// hold its fire time, and there is none. Its stored ticks then resolve further
    /// than they used to, by at most ½ `MIN_SLOT`. The alternative is to rewrite the
    /// stored nudge, which ADR 0007 §4 forbids outright — a nudge is absolute and
    /// survives a geometry change unscaled — so the fire time yields and the bound is
    /// what is asserted, in
    /// `a_nudge_at_its_slot_clamp_is_re_expressed_within_half_a_min_slot`.
    ///
    /// Exactness, which is the whole claim of this door: a hit in a slot the edit
    /// does not reshape keeps its triple **bit-for-bit** — the markers either side of
    /// it are the same `f64` values, so [`Grid::locate`] returns the same slot, the
    /// `f64` fraction it recomputes rounds back to the same `f32`, and its fire time
    /// is `f64`-equal to what it was. Only the hits inside the split or merged span
    /// are genuinely re-derived, and there the rebuild is exact whenever the new
    /// geometry can name the position in an `f32` fraction and correct to half an
    /// `f32` ulp of the slot (~8 ns at 120 bpm) when it cannot. `f` is `f32` storage;
    /// that bound is the storage's, not the mapping's — and it is a bound **per
    /// edit**, which does not cancel: repeated random marker edits random-walk a hit
    /// by a few of them (~2.6e-7 beats over 400 edits, 0.13 µs at 120 bpm).
    ///
    /// An edit the grid **refuses** returns early instead of rebuilding against a
    /// geometry that did not change. That rebuild is very nearly the identity, but
    /// "very nearly" is not what a rejected gesture should do to a hit list.
    ///
    /// Not a bypass of [`Pattern::edit_grid`] but the second half of it: the base
    /// times have to be captured before the edit and read back before the re-sort
    /// permutes the list, so it ends in the same `canonicalise` rather than nesting.
    fn edit_grid_preserving_times(&mut self, edit: impl FnOnce(&mut Grid)) {
        let mut at = [0.0_f64; MAX_HITS];
        let mut nudged = [0.0_f64; MAX_HITS];
        for i in 0..self.n_hits {
            at[i] = self.grid_pos(i);
            nudged[i] = self.resolved_nudge(self.hit_pos(i), self.hits[i].nudge);
        }
        let before = self.grid;
        edit(&mut self.grid);
        if self.grid == before {
            return;
        }
        for i in 0..self.n_hits {
            let nudge = self.hits[i].nudge;
            let p = self.grid.locate(at[i]);
            let np = self.resolved_nudge(p, nudge);
            let mut best = p;
            if np != nudged[i] {
                // The clamp moved. Shift the grid part the other way and see whether
                // the slot that lands in gives back the fire time; a candidate is only
                // taken if it is closer, so this can never be worse than not trying.
                let q = self.grid.locate(at[i] + nudged[i] - np);
                let nq = self.resolved_nudge(q, nudge);
                let target = at[i] + nudged[i];
                let ep = (self.stored_pos_of(p) + np - target).abs();
                let eq = (self.stored_pos_of(q) + nq - target).abs();
                if eq < ep {
                    best = q;
                }
            }
            self.hits[i].beat = best.beat as u16;
            self.hits[i].sub = best.sub as u8;
            self.hits[i].f = best.frac as f32;
        }
        self.canonicalise();
    }

    /// A hit's stored triple, clamped into the live geometry the way
    /// [`Pattern::fire_beat`] clamps it.
    fn hit_pos(&self, index: usize) -> GridPos {
        let h = &self.hits[index];
        let beat = (h.beat as usize).min(self.grid.n_beats() - 1);
        let sub = (h.sub as u32).min(self.grid.subs(beat) - 1);
        GridPos { beat, sub, frac: h.f as f64 }
    }

    /// Where hit `index` sits on the grid, **before** its nudge — the forward map
    /// [`Pattern::edit_grid_preserving_times`] inverts with [`Grid::locate`].
    /// [`Pattern::fire_beat`] is this plus the resolved nudge.
    fn grid_pos(&self, index: usize) -> f64 {
        if index >= self.n_hits {
            return 0.0;
        }
        self.grid.pos_of(self.hit_pos(index))
    }

    /// Where a candidate placement would actually land once `f` has been through the
    /// `f32` it is stored in — the rounding is part of the answer, so a candidate has
    /// to be judged after it and not before.
    fn stored_pos_of(&self, at: GridPos) -> f64 {
        self.grid.pos_of(GridPos { frac: at.frac as f32 as f64, ..at })
    }

    /// `nudge` in beats, under [`Pattern::fire_beat`]'s ±½-slot clamp for the slot
    /// `at` sits in — the term that does not survive a change of slot width.
    fn resolved_nudge(&self, at: GridPos, nudge: i16) -> f64 {
        let half = 0.5 * (self.grid.sub_pos(at.beat, at.sub + 1) - self.grid.sub_pos(at.beat, at.sub));
        (nudge as f64 / TICKS_PER_BEAT).clamp(-half, half)
    }
}

/// The largest `f` below 1 — `f` is a half-open fraction, so a hit stays inside
/// its own slot however it is dragged.
const F_MAX: f32 = 1.0 - f32::EPSILON;

// ── free-position edits (the continuous lane strip, ticket 0353) ──────────────
//
// Appended as its own block, clear of the slot-keyed verbs above: these are the
// editor's **hit-keyed** vocabulary, and they are what the lane strip drags,
// deletes and quantises. Position edits go through [`Pattern::remove`] +
// [`Pattern::insert`] rather than writing the array in place, so the fire-order
// invariant and the geometry clamp are re-established by exactly the code that
// already owns them, and the hit's new index comes back to the caller.
//
// Snap and quantise are **editor verbs, not storage constraints** (ADR 0007 §1):
// a quantised hit is one whose stored `f` happens to be zero, and nothing here
// records that it got there by quantising rather than by being dropped on the
// marker.

/// The lane's Y-centre while the groove carries no control points.
///
/// ADR 0007 §6 puts Y-centre points on the beat markers and interpolates between
/// them; 0350 stores them and 0356 edits them. Until then the curve is flat, so Y
/// reads as absolute-in-lane and a quantise-Y pulls to the middle of the strip.
pub const Y_CENTRE: f32 = 0.5;

impl Pattern {
    /// Move hit `index` to `(beat, sub)` with in-slot offset `(f, nudge)`,
    /// returning its **new** fire-order index (the move can reorder the lane).
    ///
    /// This is the drag verb, and it sets the whole position at once because a
    /// drag across a beat marker changes `(beat, sub)` and `f` together — setting
    /// one and then the other would resolve, in between, to a time nobody asked
    /// for.
    pub fn set_position(
        &mut self,
        index: usize,
        beat: u16,
        sub: u8,
        f: f32,
        nudge: i16,
    ) -> Option<usize> {
        if index >= self.n_hits {
            return None;
        }
        let mut h = self.hits[index];
        h.beat = beat;
        h.sub = sub;
        h.f = f;
        h.nudge = nudge;
        // Remove first, so the re-insert cannot be the one that hits MAX_HITS.
        self.remove(index);
        self.insert(h)
    }

    /// Quantise hit `index` in X toward its nearest subdivision marker by
    /// `amount ∈ [0, 1]`, returning its new fire-order index.
    ///
    /// A partial amount **lerps `f` toward 0 or 1** — whichever marker is nearer —
    /// and decays `nudge` by the same fraction, so a half-quantise halves both the
    /// proportional and the absolute part of a hit's lateness. At `amount = 1` the
    /// hit is stored *on* the marker (`f = 0`, no nudge) rather than at `f ≈ 1`
    /// against the marker before it: welding is what makes a snapped hit survive a
    /// later groove edit (ADR 0007 §4), and `f ≈ 1` does not weld.
    ///
    /// The pattern end is not a storable position — a hit there would have no
    /// owning slot (ADR 0007 §2) — so **the lane's final slot has no forward
    /// target** and a hit in it quantises backwards whatever its fraction. The
    /// partial branch honours that too: lerping toward a marker the full quantise
    /// will never land on would drag the hit later and later up the amount
    /// control, then snap it a whole slot earlier at the top of it.
    ///
    /// The choice is made on `f` alone. `nudge` is the absolute part and decays on
    /// its own; letting it flip which marker is "nearest" would make a flam decide
    /// the slot, which is the opposite of what the split is for (ADR 0007 §4).
    pub fn quantise_x(&mut self, index: usize, amount: f32) -> Option<usize> {
        if index >= self.n_hits {
            return None;
        }
        let a = if amount.is_finite() { amount.clamp(0.0, 1.0) } else { 0.0 };
        let h = self.hits[index];
        let next = self.next_marker(h.beat, h.sub);
        let toward_next = h.f > 0.5 && next.is_some();
        if a >= 1.0 {
            let (b, k) = match (toward_next, next) {
                (true, Some(m)) => m,
                _ => (h.beat, h.sub),
            };
            return self.set_position(index, b, k, 0.0, 0);
        }
        let f = if toward_next { h.f + (1.0 - h.f) * a } else { h.f * (1.0 - a) };
        let nudge = (h.nudge as f32 * (1.0 - a)).round() as i16;
        self.set_position(index, h.beat, h.sub, f, nudge)
    }

    /// Quantise hit `index` in Y toward the groove's centre curve by
    /// `amount ∈ [0, 1]`.
    ///
    /// Independent of [`Pattern::quantise_x`], and deliberately so: X and Y are not
    /// stored symmetrically (ADR 0007 §6), so they are not corrected together. No
    /// re-sort — Y cannot move a hit in time.
    pub fn quantise_y(&mut self, index: usize, amount: f32) {
        if index >= self.n_hits {
            return;
        }
        let a = if amount.is_finite() { amount.clamp(0.0, 1.0) } else { 0.0 };
        let y = self.hits[index].y;
        let c = self.y_centre(index);
        self.hits[index].y = y + (c - y) * a;
    }

    /// The groove's Y-centre curve sampled at hit `index` (ADR 0007 §6). Flat at
    /// [`Y_CENTRE`] until 0350 puts control points on the beat markers — every
    /// reader goes through here so that lands as one change.
    #[inline]
    pub fn y_centre(&self, _index: usize) -> f32 {
        Y_CENTRE
    }

    /// Set a hit's position on the lane's modulation axis, clamped to the strip.
    pub fn set_hit_y(&mut self, index: usize, y: f32) {
        if index < self.n_hits {
            self.hits[index].y = if y.is_finite() { y.clamp(0.0, 1.0) } else { Y_CENTRE };
        }
    }

    /// Set a hit's note + velocity. Hit-keyed, unlike [`Pattern::set`]: reassigning
    /// a lane's voice has to re-pitch every hit it holds, and several hits can
    /// share one slot.
    pub fn set_hit_note(&mut self, index: usize, note: f32, velocity: f32) {
        if index < self.n_hits {
            self.hits[index].note = note;
            self.hits[index].velocity = velocity;
        }
    }

    /// Set a hit's fire probability, keyed by hit rather than by slot.
    pub fn set_hit_probability(&mut self, index: usize, probability: f32) {
        if index < self.n_hits {
            self.hits[index].probability = probability;
        }
    }

    /// Set a hit's retrig macro, keyed by hit rather than by slot.
    pub fn set_hit_retrig(&mut self, index: usize, retrig: Retrig) {
        if index < self.n_hits {
            self.hits[index].retrig = retrig;
        }
    }

    /// The subdivision marker after `(beat, sub)`, or `None` when that is the
    /// lane's last one — the end marker is a bound, not a slot, so there is
    /// nothing after it to quantise or snap to.
    fn next_marker(&self, beat: u16, sub: u8) -> Option<(u16, u8)> {
        let b = (beat as usize).min(self.grid.n_beats() - 1);
        let k = (sub as u32).min(self.grid.subs(b) - 1);
        if k + 1 < self.grid.subs(b) {
            Some((b as u16, (k + 1) as u8))
        } else if b + 1 < self.grid.n_beats() {
            Some(((b + 1) as u16, 0))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{MAX_SUBS, Swing};

    /// The xorshift32 the codebase already uses, seeded explicitly so a
    /// property-test failure reproduces exactly.
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
        fn below(&mut self, n: u32) -> u32 {
            self.next_u32() % n.max(1)
        }
    }

    #[test]
    fn nudge_ticks_are_half_a_min_slot() {
        assert_eq!(MAX_NUDGE_TICKS as f64 / TICKS_PER_BEAT, MIN_SLOT * 0.5);
    }

    #[test]
    fn default_lane_is_empty_on_the_old_sixteen_step_grid() {
        let p = Pattern::default();
        assert!(p.is_empty());
        assert_eq!(p.total_subs(), 16);
        assert_eq!(p.grid().len_beats(), 4.0);
    }

    /// AC: `f = 0` hits land **exactly** on their subdivision marker at every
    /// swing amount — `f64` equality against `sub_pos`, not an epsilon.
    #[test]
    fn welded_hits_sit_exactly_on_their_marker_at_every_swing() {
        for subs in [1u32, 2, 3, 4, 6, 8] {
            for i in 0..24 {
                let amount = -1.0 + 2.0 * (i as f64 / 23.0);
                let mut p = Pattern::default();
                p.edit_grid(|g| {
                    g.set_default_subs(subs);
                    g.set_swing(Swing::mpc(amount));
                    g.set_beat_marker(1, 0.8);
                    g.set_beat_subs(2, Some(3)); // a tuplet in the mix
                });
                for slot in 0..p.total_subs() {
                    p.set(slot as usize, 36.0, 1.0);
                }
                for i in 0..p.len() {
                    let h = p.hits()[i];
                    assert_eq!(
                        p.fire_beat(i),
                        p.grid().sub_pos(h.beat as usize, h.sub as u32),
                        "subs={subs} amount={amount} hit={i}"
                    );
                }
            }
        }
    }

    /// AC: `nudge` is unscaled by swing — the same absolute offset from the
    /// marker as the slot around it lengthens.
    #[test]
    fn nudge_does_not_scale_with_the_slot() {
        // Small enough that the ±½-slot resolve clamp cannot bind at either swing.
        let ticks = 60_i16;
        let want = ticks as f64 / TICKS_PER_BEAT;
        let mut straight = Pattern::default();
        straight.edit_grid(|g| g.set_default_subs(2));
        straight.set(1, 36.0, 1.0);
        straight.set_offset(0, 0.0, ticks);

        let mut swung = straight;
        swung.edit_grid(|g| g.set_swing(Swing::mpc(0.9)));

        let marker = |p: &Pattern| {
            let h = p.hits()[0];
            p.grid().sub_pos(h.beat as usize, h.sub as u32)
        };
        // The marker really did move and the slot really did change width —
        // otherwise the test proves nothing.
        assert!(marker(&swung) > marker(&straight));
        assert!((swung.slot_span(0) - straight.slot_span(0)).abs() > 0.1);
        assert_eq!(straight.fire_beat(0) - marker(&straight), want);
        assert!((swung.fire_beat(0) - marker(&swung) - want).abs() < 1e-15);
    }

    /// AC: `nudge` is clamped to ±½ `MIN_SLOT` on every write path.
    #[test]
    fn nudge_is_clamped_on_every_write_path() {
        let mut p = Pattern::default();
        p.insert(Hit { nudge: 30_000, ..Hit::at(0, 1) });
        assert_eq!(p.hits()[0].nudge, MAX_NUDGE_TICKS);
        p.set_offset(0, 0.25, -30_000);
        assert_eq!(p.hits()[0].nudge, -MAX_NUDGE_TICKS);
        // And `f` cannot leave its slot either.
        p.set_offset(0, 4.0, 0);
        assert!(p.hits()[0].f < 1.0);
        p.set_offset(0, f32::NAN, 0);
        assert_eq!(p.hits()[0].f, 0.0);
    }

    /// AC: over randomised placements, nudges and swing amounts the resolved fire
    /// times are non-decreasing in hit order.
    ///
    /// The list *is* the fire order, so this pins that every write path
    /// re-establishes it — a missing re-sort after a grid edit shows up here and,
    /// per E050's risk note, nowhere else.
    #[test]
    fn resolved_fire_times_are_non_decreasing_in_hit_order() {
        let mut rng = Rng(0xF00D_0348);
        for trial in 0..400 {
            let mut p = Pattern::default();
            let subs = 1 + rng.below(MAX_SUBS);
            let beats = 1 + rng.below(8) as usize;
            p.set_grid_beats(beats);
            p.set_grid_subs(subs);
            let total = p.total_subs();
            // Place hits at random slots, with random in-slot offsets.
            for _ in 0..24 {
                let slot = rng.below(total);
                let (b, k) = p.grid().sub_of_index(slot);
                p.insert(Hit {
                    f: rng.unit() as f32,
                    nudge: (rng.bipolar() * 2.0 * MAX_NUDGE_TICKS as f64) as i16,
                    ..Hit::at(b as u16, k as u8)
                });
            }
            // Then re-time everything underneath them.
            p.edit_grid(|g| {
                g.set_swing(Swing::mpc(rng.bipolar()));
                g.set_beat_marker(1, rng.unit() * beats as f64);
                g.set_beat_subs(trial % beats, Some(1 + rng.below(MAX_SUBS)));
            });
            let mut prev = f64::NEG_INFINITY;
            for i in 0..p.len() {
                let t = p.fire_beat(i);
                assert!(
                    t >= prev,
                    "trial {trial}: hit {i} fires at {t}, behind {prev} (subs={subs} beats={beats})"
                );
                prev = t;
            }
            // And the global form stays ordered across the loop wrap.
            let n = p.len() as i64;
            let mut prev = f64::NEG_INFINITY;
            for g in -n..2 * n {
                let t = p.fire_beat_at(g);
                assert!(t >= prev, "trial {trial}: global hit {g} out of order");
                prev = t;
            }
        }
    }

    /// A hit's fire time stays inside the `[-½, +1½)`-slot displacement bound the
    /// lookahead window is sized from, however narrow the subdivision.
    #[test]
    fn fire_time_stays_within_the_window_bound() {
        let mut rng = Rng(0x0348_BEEF);
        for _ in 0..200 {
            let mut p = Pattern::default();
            p.set_grid_subs(1 + rng.below(MAX_SUBS));
            p.edit_grid(|g| g.set_swing(Swing::mpc(rng.bipolar())));
            let total = p.total_subs();
            for _ in 0..8 {
                let (b, k) = p.grid().sub_of_index(rng.below(total));
                p.insert(Hit {
                    f: rng.unit() as f32,
                    nudge: (rng.bipolar() * 2.0 * MAX_NUDGE_TICKS as f64) as i16,
                    ..Hit::at(b as u16, k as u8)
                });
            }
            for i in 0..p.len() {
                let h = p.hits()[i];
                let marker = p.grid().sub_pos(h.beat as usize, h.sub as u32);
                let span = p.slot_span(i);
                let off = p.fire_beat(i) - marker;
                assert!(off >= -0.5 * span - 1e-12, "early by {off} of span {span}");
                assert!(off < 1.5 * span + 1e-12, "late by {off} of span {span}");
            }
        }
    }

    #[test]
    fn over_capacity_inserts_drop_rather_than_grow() {
        let mut p = Pattern::default();
        p.set_grid_beats(16);
        p.set_grid_subs(16);
        for slot in 0..p.total_subs() {
            p.set(slot as usize, 36.0, 1.0);
        }
        assert_eq!(p.len(), MAX_HITS, "the lane fills to its ceiling and stops");
        assert_eq!(p.insert(Hit::at(0, 0)), None);
    }

    #[test]
    fn locks_travel_with_their_hit_through_a_resort() {
        let mut p = Pattern::default();
        p.set(4, 36.0, 1.0);
        p.set(0, 36.0, 1.0);
        // Fire order, so the beat-0 hit sorts first however it was added.
        assert_eq!(p.hits()[0].beat, 0);
        p.set_lock(1, LockParam::Gain, Lock { value: 0.3, termination: Termination::Latch });
        // Push the beat-0 hit to the very end of its slot, then widen that slot to
        // the whole beat so its nudge carries it past the beat-1 hit.
        p.set_offset(0, 1.0, MAX_NUDGE_TICKS);
        p.edit_grid(|g| g.set_beat_subs(0, Some(1)));
        assert_eq!(p.hits()[0].beat, 1, "the pair swapped over");
        let locked = p.hits().iter().position(|h| h.locks[0].is_some()).unwrap();
        assert_eq!(locked, 0, "the lock followed its hit, not its index");
        assert_eq!(p.lock_at(locked, 0).map(|l| l.value), Some(0.3));
    }

    #[test]
    fn slot_edits_add_remove_and_find() {
        let mut p = Pattern::default();
        p.toggle(5);
        assert_eq!(p.len(), 1);
        assert_eq!(p.hit_index_at_slot(5), Some(0));
        p.set_probability(5, 0.25);
        assert_eq!(p.hits()[0].probability, 0.25);
        p.toggle(5);
        assert!(p.is_empty());
        // A slot edit on an empty slot creates the hit it needs.
        p.set_retrig(9, Retrig { n: 3, m: 2, curve: RetrigCurve::Even, vel_end: 0.5 });
        assert_eq!(p.len(), 1);
        assert_eq!(p.hits()[0].retrig.n, 3);
        p.clear(9);
        assert!(p.is_empty());
    }

    /// A slot the lane does not have is a no-op, not an alias for the last one —
    /// an editor cell left over from a longer lane must not edit a hit it is not
    /// pointing at.
    #[test]
    fn slots_past_the_lane_end_are_a_no_op() {
        let mut p = Pattern::default();
        p.set(15, 36.0, 1.0);
        p.set_grid_beats(1); // 4 live slots; the hit re-snaps onto the last of them
        assert_eq!(p.len(), 1);
        assert_eq!(p.total_subs(), 4);
        let before = p.hits()[0];

        p.set(15, 99.0, 0.1);
        p.toggle(15);
        p.clear(15);
        p.set_probability(15, 0.0);
        p.set_retrig(15, Retrig { n: 8, m: 8, curve: RetrigCurve::Even, vel_end: 0.0 });
        assert_eq!(p.hit_index_at_slot(15), None);
        assert_eq!(p.len(), 1, "no phantom hit was created off the end");
        assert_eq!(p.hits()[0], before, "and the last live slot was left alone");
    }

    #[test]
    fn polymeter_is_per_lane_beat_count() {
        let mut short = Pattern::default();
        short.set_grid_beats(3);
        assert_eq!(short.grid().len_beats(), 3.0);
        assert_eq!(short.total_subs(), 12);
        let long = Pattern::default();
        assert_eq!(long.grid().len_beats(), 4.0);
        // Same slot, different absolute time on the second pass — they phase.
        short.set(0, 36.0, 1.0);
        assert_eq!(short.fire_beat_at(1), 3.0);
    }

    #[test]
    fn retrig_span_follows_the_grid() {
        let mut p = Pattern::default();
        p.set(0, 36.0, 1.0);
        assert_eq!(p.retrig_span(0, 2), 0.5, "two straight 16ths");
        p.edit_grid(|g| g.set_beat_subs(0, Some(2)));
        assert_eq!(p.retrig_span(0, 2), 1.0, "two slots of a halved beat");
    }

    // ── marker edit semantics (ADR 0007 §5, ticket 0349) ──────────────────────

    /// Fire time of the hit tagged `note`.
    ///
    /// The list *is* the fire order, so any comparison across an edit has to follow
    /// the tag rather than the index: indexing both sides is only sound while the
    /// property under test holds, which is exactly the case where a test must not be
    /// trusted. Every fixture below tags its hits with distinct notes for this.
    fn fire_of(p: &Pattern, note: f32) -> f64 {
        p.fire_beat(index_of(p, note))
    }

    /// Width of the slot the hit tagged `note` sits in.
    fn span_of(p: &Pattern, note: f32) -> f64 {
        p.slot_span(index_of(p, note))
    }

    fn index_of(p: &Pattern, note: f32) -> usize {
        p.hits()
            .iter()
            .position(|h| h.note == note)
            .unwrap_or_else(|| panic!("hit {note} lost"))
    }

    /// A lane of welded 16ths plus one deliberately off-grid hit in each of the two
    /// slots either side of `m[1]`, tagged by note so a hit can be found again after
    /// a re-sort.
    fn lane_across_the_first_marker() -> Pattern {
        let mut p = Pattern::default();
        for slot in 0..8 {
            p.set(slot, slot as f32, 1.0);
        }
        p.insert(Hit { f: 0.4, note: 90.0, ..Hit::at(0, 2) });
        p.insert(Hit { f: 0.6, note: 91.0, ..Hit::at(1, 1) });
        p
    }

    /// AC: dragging `m[i]` moves the hits in slot `i-1` **and** slot `i` at once, in
    /// proportion to their slots, and writes no hit record. That last part is what
    /// makes undo one stored value however many hits appear to move.
    #[test]
    fn a_marker_drag_rubber_bands_both_slots_and_writes_no_hit() {
        let mut p = lane_across_the_first_marker();
        let before = p;
        assert_eq!(p.drag_beat_marker(1, 1.25), 1.25);

        // Record-for-record, followed by tag: a drag may *permute* the list, since
        // an absolute nudge can carry a hit past its neighbour when a slot narrows.
        // What it must never do is rewrite one.
        assert_eq!(p.len(), before.len());
        let (mut moved_left, mut moved_right) = (false, false);
        for h in before.hits() {
            assert_eq!(&p.hits()[index_of(&p, h.note)], h, "a drag wrote a hit record");
            let (was, now) = (fire_of(&before, h.note), fire_of(&p, h.note));
            if was < 1.0 {
                // Slot 0 stretched from [0, 1] to [0, 1.25]: everything in it scales.
                assert!((now - was * 1.25).abs() < 1e-12, "hit {} at {was} -> {now}", h.note);
                moved_left |= now != was;
            } else {
                // Slot 1 squashed from [1, 2] to [1.25, 2] at the same time.
                let u = was - 1.0;
                assert!((now - (1.25 + u * 0.75)).abs() < 1e-12, "hit {} at {was} -> {now}", h.note);
                moved_right |= now != was;
            }
        }
        assert!(moved_left && moved_right, "one grab must move hits on both sides");
    }

    /// AC: undo of a marker drag restores one value and every apparent hit position —
    /// `f64` equality on the times, not an epsilon.
    #[test]
    fn undoing_a_marker_drag_restores_every_hit_position() {
        let mut p = lane_across_the_first_marker();
        p.edit_grid(|g| g.set_swing(Swing::mpc(0.6)));
        let before = p;
        let undo = p.grid().beat_marker(1); // the whole of the undo record

        p.drag_beat_marker(1, 1.6);
        assert!(
            before.hits().iter().any(|h| fire_of(&p, h.note) != fire_of(&before, h.note)),
            "the drag has to have moved something for the restore to mean anything"
        );

        assert_eq!(p.drag_beat_marker(1, undo), undo);
        assert_eq!(p.grid(), before.grid());
        for h in before.hits() {
            assert_eq!(fire_of(&p, h.note), fire_of(&before, h.note), "hit {}", h.note);
        }
    }

    /// AC: inserting a beat marker changes no hit's absolute fire time — `f64`
    /// equality on the resolved times before and after.
    #[test]
    fn inserting_a_marker_preserves_every_absolute_fire_time() {
        let mut p = Pattern::default();
        for slot in 0..16 {
            p.set(slot, slot as f32, 1.0);
        }
        p.insert(Hit { f: 0.5, note: 90.0, ..Hit::at(1, 1) });
        let before = p;

        assert_eq!(p.insert_beat_marker(2, 1.5), Some(2), "split beat slot 1 in two");
        assert_eq!(p.grid().n_beats(), 5);
        assert_eq!(p.len(), before.len(), "an insert neither adds nor drops a hit");
        for h in before.hits() {
            assert_eq!(fire_of(&p, h.note), fire_of(&before, h.note), "hit {}", h.note);
        }
        // The hits in the split slot really were re-derived — the ones above it rode
        // up a beat index, and the ones inside it changed subdivision.
        assert_ne!(p.hits(), before.hits());
    }

    /// AC: deleting a beat marker changes no hit's absolute fire time either.
    #[test]
    fn deleting_a_marker_preserves_every_absolute_fire_time() {
        let mut p = Pattern::default();
        for slot in 0..16 {
            p.set(slot, slot as f32, 1.0);
        }
        p.insert(Hit { f: 0.5, note: 90.0, ..Hit::at(1, 1) });
        let before = p;

        assert!(p.delete_beat_marker(2), "merge beat slots 1 and 2");
        assert_eq!(p.grid().n_beats(), 3);
        assert_eq!(p.len(), before.len());
        for h in before.hits() {
            assert_eq!(fire_of(&p, h.note), fire_of(&before, h.note), "hit {}", h.note);
        }
        assert_ne!(p.hits(), before.hits());
    }

    /// A hit welded to the marker being deleted lands at a **non-zero `f`** in the
    /// merged slot, which is correct: what a delete preserves is where the hit is,
    /// not that it happened to be snapped.
    #[test]
    fn a_hit_welded_to_a_deleted_marker_keeps_its_time_not_its_snap() {
        let mut p = Pattern::default();
        p.set_grid_subs(1); // the beat markers are then the only markers there are
        p.set(1, 36.0, 1.0);
        assert_eq!(p.hits()[0].f, 0.0, "welded to m[1]");
        assert_eq!(p.fire_beat(0), 1.0);

        assert!(p.delete_beat_marker(1));
        assert_eq!(p.fire_beat(0), 1.0, "the time is what survives");
        assert_eq!(p.hits()[0].beat, 0);
        assert_eq!(p.hits()[0].f, 0.5, "and it is no longer on a marker");
    }

    /// AC: delete-then-insert at the same position is a no-op **on fire times**. The
    /// stored triples are not what is asserted — nothing requires them to round-trip,
    /// and in the swung half below they do not.
    #[test]
    fn delete_then_insert_at_the_same_place_is_a_no_op_on_fire_times() {
        let mut p = Pattern::default();
        for slot in 0..16 {
            p.set(slot, slot as f32, 1.0);
        }
        p.insert(Hit { f: 0.5, note: 90.0, ..Hit::at(2, 3) });
        let before = p;
        let pos = p.grid().beat_marker(2);

        assert!(p.delete_beat_marker(2));
        assert_eq!(p.insert_beat_marker(2, pos), Some(2));
        assert_eq!(p.grid(), before.grid(), "the geometry came back exactly");
        for h in before.hits() {
            assert_eq!(fire_of(&p, h.note), fire_of(&before, h.note), "hit {}", h.note);
        }

        // The same round trip on geometry that is not dyadic, where the rebuilt `f32`
        // fraction cannot always name the position exactly. The time is still held to
        // the resolution of that fraction, which is ~10 ns at 120 bpm.
        let mut p = Pattern::default();
        p.edit_grid(|g| {
            g.set_swing(Swing::mpc(0.45));
            g.set_beat_marker(1, 0.7);
            g.set_beat_marker(2, 1.9);
            g.set_beat_subs(3, Some(3)); // a tuplet clear of the merge, which rides the shift
        });
        for slot in 0..p.total_subs() {
            p.set(slot as usize, slot as f32, 1.0);
        }
        p.insert(Hit { f: 0.37, note: 90.0, ..Hit::at(1, 1) });
        let before = p;
        let pos = p.grid().beat_marker(2);

        assert!(p.delete_beat_marker(2));
        let merged = p;
        assert_eq!(p.insert_beat_marker(2, pos), Some(2));
        assert_eq!(p.grid(), before.grid());
        for h in before.hits() {
            let (was, now) = (fire_of(&before, h.note), fire_of(&p, h.note));
            // Half an `f32` ulp of the slot each rebuild lands in — the merged slot
            // the first one lands in, then the restored slot the second returns to.
            // That sum is the whole budget for the round trip.
            let tol =
                0.5 * f32::EPSILON as f64 * (span_of(&merged, h.note) + span_of(&p, h.note));
            assert!((now - was).abs() <= tol, "hit {}: {was} -> {now} (tol {tol})", h.note);
        }
    }

    /// AC: the outer markers reject drag, delete and insert-outside — at the pattern
    /// level too, where a refused edit must also leave every hit alone.
    #[test]
    fn outer_markers_reject_every_marker_edit() {
        let mut p = lane_across_the_first_marker();
        p.edit_grid(|g| g.set_swing(Swing::mpc(0.3)));
        let before = p;

        assert_eq!(p.drag_beat_marker(0, 0.5), 0.0);
        assert_eq!(p.drag_beat_marker(4, 9.0), 4.0);
        assert_eq!(p.insert_beat_marker(0, 0.5), None);
        assert_eq!(p.insert_beat_marker(1, 0.0), None, "on the pinned start");
        assert_eq!(p.insert_beat_marker(5, 4.5), None, "past the pinned end");
        assert!(!p.delete_beat_marker(0));
        assert!(!p.delete_beat_marker(4));

        assert_eq!(p.grid(), before.grid());
        // A refused edit does not reach the rebuild at all — the sandwich returns
        // early on unchanged geometry — so this is record-for-record, not near-miss.
        assert_eq!(p.hits(), before.hits());
        for h in before.hits() {
            assert_eq!(fire_of(&p, h.note), fire_of(&before, h.note), "hit {}", h.note);
        }
    }

    /// AC: an insert or a delete changes no hit's absolute fire time, over randomised
    /// geometry rather than the hand-picked dyadic cases above.
    ///
    /// Two standards, and the split is the storage's rather than the mapping's. A hit
    /// in a slot the edit does not reshape is held to **`f64` equality** — its markers
    /// are the same values, so its triple survives bit-for-bit, and so does its nudge
    /// clamp. A hit inside the split or merged span is rebuilt through an `f32`
    /// fraction and so is held to one `f32` ulp of its slot (about 8 ns at a 16th of
    /// 120 bpm), plus whatever of its stored nudge the new slot width releases.
    ///
    /// Nudges are deliberately in the mix and deliberately over-range: the `f32`
    /// bound alone would be a weaker claim than it looks if the nudge were held at
    /// zero, since it is the nudge's ±½-slot resolve clamp that can move a hit by
    /// milliseconds rather than nanoseconds.
    #[test]
    fn marker_edits_preserve_fire_times_over_random_geometry() {
        let mut rng = Rng(0x0349_0002);
        let mut inserts = 0;
        let mut deletes = 0;
        for trial in 0..200 {
            let mut p = Pattern::default();
            p.set_grid_beats(2 + rng.below(6) as usize);
            p.set_grid_subs(1 + rng.below(MAX_SUBS));
            p.edit_grid(|g| {
                g.set_swing(Swing::mpc(rng.bipolar()));
                g.set_beat_marker(1, rng.unit() * 2.0);
                g.set_beat_subs(0, Some(1 + rng.below(MAX_SUBS)));
            });
            let total = p.total_subs();
            for tag in 0..12 {
                let (b, k) = p.grid().sub_of_index(rng.below(total));
                p.insert(Hit {
                    f: rng.unit() as f32,
                    nudge: (rng.bipolar() * 2.0 * MAX_NUDGE_TICKS as f64) as i16,
                    note: tag as f32,
                    ..Hit::at(b as u16, k as u8)
                });
            }
            let before = p;

            let n = p.grid().n_beats() as u32;
            let i = 1 + rng.below(n - 1) as usize;
            // The beats whose shape actually changes: the split one, or the two that
            // merge. Every other hit is held to the stronger standard.
            let (lo, hi) = if trial % 2 == 0 {
                let (a, b) = (p.grid().beat_marker(i - 1), p.grid().beat_marker(i));
                if p.insert_beat_marker(i, a + rng.unit() * (b - a)).is_none() {
                    continue;
                }
                inserts += 1;
                (i - 1, i - 1)
            } else {
                if !p.delete_beat_marker(i) {
                    continue;
                }
                deletes += 1;
                (i - 1, i)
            };

            assert_eq!(p.len(), before.len());
            for h in before.hits() {
                let (was, now) = (fire_of(&before, h.note), fire_of(&p, h.note));
                let beat = h.beat as usize;
                if beat < lo || beat > hi {
                    assert_eq!(now, was, "trial {trial}: untouched beat {beat} moved");
                } else {
                    // Half an f32 ulp of the rebuilt fraction, plus however much of
                    // the stored nudge the new slot width lets through that the old
                    // one did not — the residual the sandwich cannot always place.
                    let ticks = h.nudge as f64 / TICKS_PER_BEAT;
                    let (sb, sa) = (span_of(&before, h.note), span_of(&p, h.note));
                    let nb = ticks.clamp(-0.5 * sb, 0.5 * sb);
                    let na = ticks.clamp(-0.5 * sa, 0.5 * sa);
                    let tol = f32::EPSILON as f64 * sa + (na - nb).abs();
                    assert!((now - was).abs() <= tol, "trial {trial}: {was} -> {now}");
                }
            }
        }
        assert!(inserts > 50 && deletes > 50, "{inserts} inserts, {deletes} deletes");
    }

    /// A large nudge is re-expressed against the slot it lands in, and the sandwich
    /// compensates for that wherever there is room to — which is everywhere except a
    /// hit whose compensating position would fall outside the pattern.
    ///
    /// The residual is bounded by ½ `MIN_SLOT` (3.9 ms at 120 bpm), reachable only by
    /// a hit whose stored nudge exceeds half its own slot, and it is the *resolve*
    /// clamp of 0348 releasing rather than a position being lost: the alternative is
    /// to rewrite the stored ticks, which ADR 0007 §4 forbids — a nudge is absolute
    /// and survives a geometry change unscaled. Swept rather than asserted at one
    /// point, because the bound is the claim.
    #[test]
    fn a_nudge_at_its_slot_clamp_is_re_expressed_within_half_a_min_slot() {
        let mut worst = 0.0_f64;
        let mut compensated = 0;
        for subs in 1..=MAX_SUBS {
            for w in [0.02, 0.05, 0.1, 0.2, 0.5, 1.0] {
                for k in 0..subs.min(4) {
                    let mut p = Pattern::default();
                    p.set_grid_subs(subs);
                    p.edit_grid(|g| {
                        g.set_beat_marker(1, w);
                    });
                    p.insert(Hit { nudge: MAX_NUDGE_TICKS, ..Hit::at(0, k as u8) });
                    let was = p.fire_beat(0);
                    for mut q in [p, p] {
                        let moved = if q.delete_beat_marker(1) {
                            true
                        } else {
                            q.insert_beat_marker(1, w * 0.5).is_some()
                        };
                        if !moved {
                            continue;
                        }
                        let d = (q.fire_beat(0) - was).abs();
                        compensated += usize::from(d < 1e-9);
                        worst = worst.max(d);
                    }
                }
            }
        }
        assert!(worst <= 0.5 * MIN_SLOT, "residual {worst} exceeds half a MIN_SLOT");
        assert!(compensated > 100, "only {compensated} placements came out exact");
    }

    /// Merging two slots can leave two hits sharing one `(beat, sub)` — they keep
    /// their distinct absolute times, which is the whole point, but the *slot-keyed*
    /// verbs then only reach the first of them.
    ///
    /// Not new and not a defect: `Pattern::canonical` already says a re-snap can stack
    /// hits on one slot (0348). Recorded here because a delete is the gesture that
    /// makes it easy to do on purpose, and because the continuous editor addresses
    /// hits by index, where they are two perfectly distinct diamonds.
    #[test]
    fn merging_slots_can_stack_hits_that_the_slot_keyed_verbs_cannot_separate() {
        let mut p = Pattern::default();
        p.set_grid_subs(1);
        p.set(0, 36.0, 1.0);
        p.set(1, 38.0, 1.0);
        assert!(p.delete_beat_marker(1));
        assert_eq!(p.len(), 2, "both hits survive the merge");
        assert_eq!(p.fire_beat(0), 0.0);
        assert_eq!(p.fire_beat(1), 1.0, "and both keep their absolute time");
        assert_eq!((p.hits()[0].beat, p.hits()[0].sub), (0, 0));
        assert_eq!((p.hits()[1].beat, p.hits()[1].sub), (0, 0), "same slot now");
        // The slot-keyed path reaches the first only; index-keyed reaches both.
        p.clear(0);
        assert_eq!(p.len(), 1);
        p.remove(0);
        assert!(p.is_empty());
    }

    /// AC (the one that guards hardest): randomised marker edits interleaved with
    /// randomised hit placements never produce out-of-order fire times, a degenerate
    /// slot, or a NaN.
    ///
    /// A bypassed clamp gives a zero-width slot, which divides by ~0 in the inverse
    /// mapping and poisons every hit in that slot silently — nothing else here fails
    /// on it, so this looks for it explicitly.
    #[test]
    fn random_marker_edits_never_produce_a_degenerate_lane() {
        let mut rng = Rng(0x0349_0003);
        for trial in 0..300 {
            let mut p = Pattern::default();
            p.set_grid_beats(1 + rng.below(8) as usize);
            p.set_grid_subs(1 + rng.below(MAX_SUBS));
            p.edit_grid(|g| g.set_swing(Swing::mpc(rng.bipolar())));
            for _ in 0..40 {
                let n = p.grid().n_beats() as u32;
                let i = rng.below(n + 2) as usize;
                let len = p.grid().len_beats();
                // Wild positions on purpose: outside the pattern, and non-finite.
                let pos = match rng.below(8) {
                    0 => f64::NAN,
                    1 => f64::NEG_INFINITY,
                    2 => rng.bipolar() * 50.0,
                    _ => rng.unit() * len,
                };
                match rng.below(6) {
                    0 => {
                        let total = p.total_subs();
                        let (b, k) = p.grid().sub_of_index(rng.below(total));
                        p.insert(Hit {
                            f: rng.unit() as f32,
                            nudge: (rng.bipolar() * 2.0 * MAX_NUDGE_TICKS as f64) as i16,
                            ..Hit::at(b as u16, k as u8)
                        });
                    }
                    1 => {
                        p.drag_beat_marker(i, pos);
                    }
                    2 => {
                        p.insert_beat_marker(i, pos);
                    }
                    3 => {
                        p.delete_beat_marker(i);
                    }
                    4 => p.edit_grid(|g| g.set_beat_subs(i, Some(1 + rng.below(MAX_SUBS)))),
                    _ => p.edit_grid(|g| g.set_swing(Swing::mpc(rng.bipolar()))),
                }

                let g = p.grid();
                let len = g.len_beats();
                assert_eq!(g.beat_marker(0), 0.0, "trial {trial}: the start marker moved");
                for b in 0..g.n_beats() {
                    let (lo, hi) = (g.beat_marker(b), g.beat_marker(b + 1));
                    assert!(lo.is_finite() && hi.is_finite());
                    assert!(hi - lo > 0.0, "trial {trial}: degenerate beat slot {b}");
                }
                let mut prev = f64::NEG_INFINITY;
                for j in 0..p.len() {
                    let h = p.hits()[j];
                    assert!((h.beat as usize) < p.grid().n_beats(), "dead beat index");
                    assert!((h.sub as u32) < p.grid().subs(h.beat as usize), "dead sub");
                    assert!(h.f.is_finite() && (0.0..1.0).contains(&h.f), "f = {}", h.f);
                    assert!(p.slot_span(j) > 0.0, "trial {trial}: degenerate slot");
                    let t = p.fire_beat(j);
                    assert!(t.is_finite(), "trial {trial}: hit {j} fires at {t}");
                    assert!((0.0..=len).contains(&t), "trial {trial}: hit {j} at {t}");
                    assert!(t >= prev, "trial {trial}: hit {j} at {t}, behind {prev}");
                    prev = t;
                }
            }
        }
    }
}

#[cfg(test)]
mod position_tests {
    use super::*;
    use crate::grid::{GridPos, Swing};

    /// The editor's drag: resolve a pointer position through the grid, store it as
    /// `(beat, sub, f)`, and move the hit there. Exactly what `app.js` does.
    fn drag(p: &mut Pattern, index: usize, t: f64) -> usize {
        let at = p.grid().locate(t);
        p.set_position(index, at.beat as u16, at.sub as u8, at.frac as f32, 0).unwrap()
    }

    /// AC: a drag across a beat marker updates `(beat, sub)` and recomputes `f`,
    /// and the hit does not jump — the resolved time tracks the pointer to within
    /// the `f32` the fraction is stored in.
    #[test]
    fn dragging_across_a_marker_recomputes_the_fraction_without_jumping() {
        for amount in [0.0, 0.5, 1.0, -0.7] {
            let mut p = Pattern::default();
            p.edit_grid(|g| g.set_swing(Swing::mpc(amount)));
            p.set(0, 36.0, 1.0);
            let mut prev = p.fire_beat(0);
            // Sweep the pointer clean across two beat markers, one small step at a
            // time, and watch the resolved position follow it monotonically.
            for i in 0..=400 {
                let t = 0.5 + 2.0 * (i as f64 / 400.0);
                let idx = drag(&mut p, 0, t);
                let got = p.fire_beat(idx);
                assert!(
                    (got - t).abs() < 1e-6,
                    "amount={amount} t={t}: resolved {got}, a jump of {}",
                    got - t
                );
                assert!(got >= prev - 1e-6, "amount={amount} t={t}: went backwards");
                prev = got;
                // And the stored form really is the slot the pointer is over.
                let h = p.hits()[idx];
                let at = p.grid().locate(t);
                assert_eq!((h.beat as usize, h.sub as u32), (at.beat, at.sub));
            }
        }
    }

    /// A position edit re-establishes fire order, like every other write path.
    #[test]
    fn a_position_edit_resorts_the_lane() {
        let mut p = Pattern::default();
        p.set(0, 36.0, 1.0);
        p.set(8, 40.0, 1.0);
        assert_eq!(p.hits()[0].note, 36.0);
        // Drag the first hit past the second.
        let i = drag(&mut p, 0, 3.0);
        assert_eq!(i, 1, "it sorted behind the hit it passed");
        assert_eq!(p.hits()[0].note, 40.0);
        assert_eq!(p.hits()[1].note, 36.0);
        // Out-of-range indices are a no-op, not a panic.
        assert_eq!(p.set_position(9, 0, 0, 0.0, 0), None);
    }

    /// AC: full quantise-X stores the hit **on** the nearest marker — `f = 0`, no
    /// nudge — so it welds and survives a later groove edit.
    #[test]
    fn full_quantise_x_welds_to_the_nearest_marker() {
        let mut p = Pattern::default();
        p.edit_grid(|g| g.set_swing(Swing::mpc(1.0))); // uneven slots
        p.set(1, 36.0, 1.0);
        // Just past the marker → back onto it.
        p.set_offset(0, 0.2, 100);
        let i = p.quantise_x(0, 1.0).unwrap();
        assert_eq!((p.hits()[i].f, p.hits()[i].nudge), (0.0, 0));
        assert_eq!((p.hits()[i].beat, p.hits()[i].sub), (0, 1));
        assert_eq!(p.fire_beat(i), p.grid().sub_pos(0, 1), "welded, exactly");
        // Most of the way to the next → forward onto that one.
        p.set_offset(i, 0.8, -100);
        let i = p.quantise_x(i, 1.0).unwrap();
        assert_eq!((p.hits()[i].beat, p.hits()[i].sub), (0, 2));
        assert_eq!(p.fire_beat(i), p.grid().sub_pos(0, 2));
    }

    /// AC: a partial quantise lerps `f` toward the nearer marker and decays
    /// `nudge` by the same fraction.
    #[test]
    fn partial_quantise_x_lerps_the_fraction_and_decays_the_nudge() {
        let mut p = Pattern::default();
        p.set(2, 36.0, 1.0);
        p.set_offset(0, 0.4, 200);
        let i = p.quantise_x(0, 0.5).unwrap();
        assert!((p.hits()[i].f - 0.2).abs() < 1e-6, "f lerped toward 0");
        assert_eq!(p.hits()[i].nudge, 100, "nudge decayed by the same half");
        // Past the midpoint it lerps the other way, toward the next marker.
        p.set_offset(i, 0.6, -80);
        let i = p.quantise_x(i, 0.5).unwrap();
        assert!((p.hits()[i].f - 0.8).abs() < 1e-6, "f lerped toward 1");
        assert_eq!(p.hits()[i].nudge, -40);
        // Zero amount is a no-op, and a non-finite one reads as zero rather than
        // poisoning the position.
        let before = p.hits()[i];
        let i = p.quantise_x(i, 0.0).unwrap();
        assert_eq!(p.hits()[i], before);
        let i = p.quantise_x(i, f32::NAN).unwrap();
        assert_eq!(p.hits()[i], before);
    }

    /// The pattern end owns no slot, so the last slot quantises back onto its own
    /// marker rather than off the end of the lane — and **partially** quantising
    /// it moves it the same way, monotonically in the amount. Lerping toward a
    /// marker the full quantise will never reach would drag the hit later and
    /// later up the amount control, then snap it a whole slot earlier at the top.
    #[test]
    fn the_last_slot_quantises_backwards_not_off_the_end() {
        let mut p = Pattern::default();
        let last = p.total_subs() - 1;
        let (b, k) = p.grid().sub_of_index(last);
        let marker = p.grid().sub_pos(b, k);

        let mut prev = f64::INFINITY;
        for i in 0..=20 {
            let a = i as f32 / 20.0;
            let mut c = p;
            c.set(last as usize, 36.0, 1.0);
            c.set_offset(0, 0.9, 0);
            let j = c.quantise_x(0, a).unwrap();
            let t = c.fire_beat(j);
            assert!(t <= prev, "amount={a}: {t} is later than {prev} — not monotonic");
            assert!(t >= marker, "amount={a}: quantised behind its own marker");
            prev = t;
        }
        assert_eq!(prev, marker, "the full quantise lands on the marker exactly");

        p.set(last as usize, 36.0, 1.0);
        p.set_offset(0, 0.9, 0);
        let i = p.quantise_x(0, 1.0).unwrap();
        let h = p.hits()[i];
        assert_eq!((h.beat as usize, h.sub as u32), (b, k));
        assert_eq!((h.f, h.nudge), (0.0, 0));
        assert_eq!(p.fire_beat(i), marker);
    }

    /// Quantise-Y is independent of quantise-X: it pulls Y to the centre curve and
    /// leaves the hit's time — and the lane's order — alone.
    #[test]
    fn quantise_y_lerps_to_the_centre_curve_and_moves_nothing_in_time() {
        let mut p = Pattern::default();
        p.insert(Hit { y: 1.0, f: 0.3, ..Hit::at(0, 1) });
        let t = p.fire_beat(0);
        p.quantise_y(0, 0.5);
        assert!((p.hits()[0].y - 0.75).abs() < 1e-6);
        p.quantise_y(0, 1.0);
        assert_eq!(p.hits()[0].y, Y_CENTRE);
        assert_eq!(p.fire_beat(0), t, "Y is not a timing edit");
        assert_eq!(p.hits()[0].f, 0.3);
    }

    #[test]
    fn hit_keyed_attribute_edits_address_one_hit_of_a_shared_slot() {
        let mut p = Pattern::default();
        // Two hits in one slot — the slot-keyed verbs only ever reach the first.
        p.insert(Hit::at(0, 1));
        p.insert(Hit { f: 0.5, ..Hit::at(0, 1) });
        assert_eq!(p.len(), 2);
        p.set_hit_probability(1, 0.25);
        p.set_hit_retrig(1, Retrig { n: 3, m: 2, curve: RetrigCurve::Even, vel_end: 0.5 });
        p.set_hit_note(1, 48.0, 0.6);
        p.set_hit_y(1, 0.9);
        assert_eq!(p.hits()[0].probability, 1.0);
        assert_eq!(p.hits()[1].probability, 0.25);
        assert_eq!(p.hits()[1].retrig.n, 3);
        assert_eq!((p.hits()[1].note, p.hits()[1].velocity), (48.0, 0.6));
        assert_eq!(p.hits()[1].y, 0.9);
        // Out of range is ignored, and a non-finite Y falls back to the centre.
        p.set_hit_probability(9, 0.0);
        p.set_hit_y(1, f32::NAN);
        assert_eq!(p.hits()[1].y, Y_CENTRE);
    }

    /// The editor resolves a diamond's screen position through exactly the mapping
    /// the engine fires it at — which is what makes "the hit does not jump" a
    /// property of the model rather than of the drawing code.
    #[test]
    fn locate_and_pos_of_are_inverses_on_the_swung_grid() {
        let mut p = Pattern::default();
        p.edit_grid(|g| {
            g.set_default_subs(4);
            g.set_swing(Swing::mpc(1.0));
            g.set_beat_subs(2, Some(3));
        });
        for i in 0..=200 {
            let t = 4.0 * (i as f64 / 200.0);
            let at = p.grid().locate(t);
            assert!((p.grid().pos_of(at) - t).abs() < 1e-12, "t={t}");
            assert_eq!(
                p.grid().pos_of(GridPos { frac: 0.0, ..at }),
                p.grid().sub_pos(at.beat, at.sub),
                "frac = 0 is the marker itself"
            );
        }
    }
}
