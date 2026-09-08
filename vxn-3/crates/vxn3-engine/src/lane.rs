//! Per-track sequencer state and the per-block hit scheduler (ADR 0001 §2).
//!
//! Each track resolves its [`Pattern`] against the host beat clock on its **own**
//! marker geometry, so lanes with different beat counts phase (polymeter).
//!
//! **Scheduling model (0346, hit list 0348).** Fire times are points on a
//! *continuous* per-lane beat timeline, not step indices: each block advances a
//! bounded **lookahead window** over that timeline, resolves hits into fire times,
//! emits everything in the window landing in `[beat0, beat_end)`, and carries the
//! remainder to the next block. ADR 0004 §3 (retained by ADR 0006, restated by
//! ADR 0007 §9) requires this shape up front: once a hit can sit off its grid
//! position, an early-nudged hit must fire *before* its position's block is
//! reached, which a "walk the boundaries in this block and fire" loop cannot do.
//!
//! Consequences of the shape:
//!
//! - Probability is drawn **once per primary trig**, at window-resolve time; the
//!   resolve cursor ([`LaneState::next_trig_index`]) is what stops a trig whose
//!   slot straddles a block boundary being re-rolled.
//! - Retrig is not in-flight lane state: a retrig macro expands into its `n` fire
//!   times in the window when its hit resolves, and the window carries them.
//! - p-locks resolve on a **separate cursor** over the same timeline
//!   ([`LaneState::next_lock_index`]) — per *crossed* subdivision slot, independent
//!   of trigs and of the lookahead horizon (ADR 0004 §3: independent axes). That
//!   is what `Termination::Revert { n }` counts.
//! - A transport jump drops the window along with the in-flight state it replaces.
//!
//! The trig cursor counts **hits** and the lock cursor counts **slots**: since
//! 0348 the two are different things (a slot may hold no hit, or several), and the
//! hit list is stored in fire order precisely so the trig cursor can stay a single
//! monotonic integer.
//!
//! Output is a flat list of sample-accurate [`TrigEvent`]s for the block; the
//! engine slices the track's render at those offsets.

use crate::grid::Grid;
use crate::sequencer::{Hit, MAX_HITS, N_LOCK_PARAMS, Pattern, Termination};
use crate::track_engine::TrigMod;

/// A scheduled trig within a block: a sample offset + note + velocity + the modulation
/// the firing hit carries.
///
/// Distinct from [`Hit`], which is the *stored* lane position 0348 introduced.
/// This is what one hit resolves to for one block — a retrig expands into several.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TrigEvent {
    pub frame: usize,
    pub note: f32,
    pub velocity: f32,
    /// This trig's own modulation (ADR 0007 §7): the hit's colour as a macro vector,
    /// and where it landed in its swung slot. `TrigMod::default()` for a trig with no
    /// hit behind it, which carries no colour.
    pub modulation: TrigMod,
}

// ── Lookahead window sizing (ADR 0007 §9) ─────────────────────────────────────
//
// The window is bounded by the **displacement** invariant: a hit cannot leave its
// own slot far enough to reorder past a neighbour by more than half a slot. The
// in-slot fraction `f ∈ [0, 1)` keeps a hit inside its slot by construction, and
// `nudge` — the only term that can move a hit *backwards* — is clamped, in
// `Pattern::fire_beat`, to ±½ of the hit's own slot. So a hit's fire time always
// lies in `[-½, +1½)` slots of its own grid position. That is what makes the
// window const-sized, hence preallocated and alloc-free in `schedule` (and so on
// the audio thread).

/// Slots a hit's fire time may sit **before** its own grid position — the
/// ±½-slot `nudge` clamp (ADR 0007 §9). This is the ceiling on how far past the
/// block end hits must be resolved.
const MAX_EARLY_SLOTS: f64 = 0.5;

/// The early offset actually in play on this build's grid. Since 0348 a hit
/// carries a `nudge` and can fire before its marker, so this is the real bound,
/// not the zero the step model left here.
///
/// It is resolve *slack*, not a correctness requirement: because 0348 stores the
/// hit list in fire order, `schedule` walks fire times directly and cannot miss an
/// early hit however far it moved. The slack keeps the window primed a slot ahead,
/// which is what the bound above is for.
const EARLY_SLOTS: f64 = 0.5;
const _: () = assert!(EARLY_SLOTS <= MAX_EARLY_SLOTS);

/// Slots a hit's fire time may sit **after** its own grid position: the in-slot
/// fraction `f ∈ [0, 1)`, plus the ½-slot late `nudge`.
const MAX_LATE_SLOTS: f64 = 1.5;

/// Grid positions that can hold resolved-but-unfired hits at once. A hit sits
/// within `[-½, +1½)` slots of its position — a two-slot span — so at most three
/// consecutive positions can have pending hits at any instant. The assert ties
/// this to the bounds above, so widening either one fails the build here rather
/// than silently under-sizing the window.
const LOOKAHEAD_POSITIONS: usize = 3;
const _: () = assert!(
    LOOKAHEAD_POSITIONS as f64 >= MAX_EARLY_SLOTS + MAX_LATE_SLOTS + 1.0,
    "LOOKAHEAD_POSITIONS no longer covers the offset bounds it is derived from"
);

/// Fire times one grid position can expand to: a retrig's `n`, stored as `u8`.
const MAX_HITS_PER_POSITION: usize = u8::MAX as usize;

/// Lookahead window capacity: one full retrig expansion (only one retrig is ever
/// pending — a new one replaces the previous one's tail) plus every plain hit the
/// lane can hold.
///
/// Sized from [`MAX_HITS`] rather than from [`LOOKAHEAD_POSITIONS`]: since 0348 a
/// slot can hold **several** hits (they differ in their in-slot offset), so
/// "three consecutive positions" is no longer a bound on how many fire times are
/// pending. No legal pattern can overflow this one; a push beyond capacity drops
/// the hit rather than allocate, matching [`push_hit`].
const WINDOW_CAPACITY: usize = MAX_HITS_PER_POSITION + MAX_HITS;

/// A fire time resolved onto the lane's continuous timeline but not yet emitted.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Pending {
    /// Absolute position on the host beat clock — a time, not a grid index.
    beat: f64,
    note: f32,
    velocity: f32,
    /// The firing hit's own modulation, resolved with its fire time and carried to
    /// the trig unchanged (ADR 0007 §7).
    modulation: TrigMod,
    /// Came from a retrig expansion. A new retrig replaces the pending tail of
    /// the previous one (one live retrig per lane, as before 0346); plain hits
    /// are untouched by that.
    from_retrig: bool,
}

const EMPTY_PENDING: Pending = Pending {
    beat: 0.0,
    note: 0.0,
    velocity: 0.0,
    modulation: TrigMod { macros: None, lateness: 0.0 },
    from_retrig: false,
};

/// Fixed-capacity lookahead window. Inline storage, never grown — the audio
/// thread must not allocate.
#[derive(Clone)]
struct Window {
    entries: [Pending; WINDOW_CAPACITY],
    len: usize,
}

// Hand-written so a `LaneState` dump shows the live entries, not 258 slots of
// mostly-stale backing store.
impl std::fmt::Debug for Window {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(&self.entries[..self.len]).finish()
    }
}

impl Window {
    fn new() -> Self {
        Self {
            entries: [EMPTY_PENDING; WINDOW_CAPACITY],
            len: 0,
        }
    }

    #[inline]
    fn clear(&mut self) {
        self.len = 0;
    }

    /// Resolve one fire time into the window. Over capacity the hit is dropped
    /// rather than the window grown — a dropped trig beats an allocation on the
    /// audio path (same policy as [`push_hit`]).
    #[inline]
    fn push(&mut self, entry: Pending) {
        if self.len < WINDOW_CAPACITY {
            self.entries[self.len] = entry;
            self.len += 1;
        }
    }

    /// Drop the pending tail of the live retrig, keeping plain hits. Called when
    /// a new retrig resolves: a lane has one live retrig, and the new one
    /// replaces it.
    fn drop_retrig_tail(&mut self) {
        let mut keep = 0;
        for i in 0..self.len {
            let entry = self.entries[i];
            if !entry.from_retrig {
                self.entries[keep] = entry;
                keep += 1;
            }
        }
        self.len = keep;
    }

    /// Emit every window entry landing before `beat_end` into `out`, compacting
    /// the rest (in order) for the next block.
    ///
    /// Entries already behind `beat0` are *dropped*, not bunched at frame 0: the
    /// transport has run past them.
    fn emit_due(
        &mut self,
        beat0: f64,
        beat_end: f64,
        bps: f64,
        frames: usize,
        out: &mut Vec<TrigEvent>,
    ) {
        let mut keep = 0;
        for i in 0..self.len {
            let entry = self.entries[i];
            if entry.beat >= beat_end {
                // Belongs to a future block — carry it, preserving order.
                self.entries[keep] = entry;
                keep += 1;
                continue;
            }
            if entry.beat >= beat0 - 1e-9 {
                let frame = frame_of(entry.beat, beat0, bps, frames);
                push_hit(out, frame, entry.note, entry.velocity, entry.modulation);
            }
        }
        self.len = keep;
    }
}

/// Per-track sequencer state, owned by the engine (audio thread).
#[derive(Clone, Debug)]
pub struct LaneState {
    /// Per-track PRNG for probability draws (xorshift32).
    rng: u32,
    /// Beat position expected at the next block start, for jump detection.
    expected_beat: f64,
    /// Next **hit** (global index: `pass · len + i`) not yet resolved into the
    /// window. A hit resolves exactly once, which is what makes probability draw
    /// once per primary trig even when its slot straddles a block boundary.
    next_trig_index: i64,
    /// The fire time [`Self::next_trig_index`] named when it was committed.
    ///
    /// Both cursors below are indices into a numbering the *pattern* defines, and
    /// the pattern is edited from the audio thread: adding or deleting a hit
    /// changes `Pattern::len()`, a geometry edit changes `Grid::total_subs()`, and
    /// either silently redefines what an already-committed index means — by a
    /// margin that grows with how long the lane has been playing. Recording the
    /// time alongside the index is how that is caught: the time is the authority,
    /// the index is the fast path, and a mismatch costs one re-anchor instead of a
    /// lane that goes silent (or stops resolving p-locks) for minutes.
    next_trig_beat: f64,
    /// Next **subdivision slot** (global index) whose p-locks have not been
    /// applied. Tracks *crossed* slots, so it lags the trig cursor by the
    /// lookahead horizon.
    next_lock_index: i64,
    /// The position [`Self::next_lock_index`] named when it was committed — the
    /// authority for the same reason.
    next_lock_beat: f64,
    /// Fire times resolved onto the timeline but not yet emitted.
    window: Window,

    // ── p-lock resolver (per lockable param) ──
    /// Active override value, or `None` when the param falls back to base.
    override_val: [Option<f32>; N_LOCK_PARAMS],
    /// Subdivision slots left on a `Revert` hold (`0` = not reverting; a latched
    /// override also sits at `0` but keeps a `Some` override_val).
    revert_ticks: [u32; N_LOCK_PARAMS],
}

impl LaneState {
    /// `seed_index` differentiates per-track PRNG streams.
    pub fn new(seed_index: usize) -> Self {
        Self {
            // Nonzero seed required by xorshift.
            rng: (seed_index as u32).wrapping_mul(0x9E37_79B1) ^ 0x5DEE_CE66,
            expected_beat: f64::NEG_INFINITY,
            next_trig_index: 0,
            next_trig_beat: f64::NEG_INFINITY,
            next_lock_index: 0,
            next_lock_beat: f64::NEG_INFINITY,
            window: Window::new(),
            override_val: [None; N_LOCK_PARAMS],
            revert_ticks: [0; N_LOCK_PARAMS],
        }
    }

    /// Reset transport-derived phase + the lookahead window + p-lock overrides
    /// (transport stop / engine reset). The PRNG stream is left running.
    pub fn reset(&mut self) {
        self.expected_beat = f64::NEG_INFINITY;
        self.next_trig_index = 0;
        self.next_trig_beat = f64::NEG_INFINITY;
        self.next_lock_index = 0;
        self.next_lock_beat = f64::NEG_INFINITY;
        self.window.clear();
        self.override_val = [None; N_LOCK_PARAMS];
        self.revert_ticks = [0; N_LOCK_PARAMS];
    }

    /// The active p-lock override for `param_index`, or `None` to use base.
    #[inline]
    pub fn override_value(&self, param_index: usize) -> Option<f32> {
        self.override_val[param_index]
    }

    /// Advance + apply p-locks for one crossed subdivision slot. Existing reverts
    /// tick down first (so a lock set this slot isn't decremented this slot); then
    /// the locks of every hit sitting in the slot apply, superseding any in-flight
    /// hold (preemption, no queue).
    ///
    /// A slot may hold no hit at all, or more than one (hits differing only in
    /// their in-slot offset) — the revert countdown is a property of the *grid*,
    /// so it advances either way.
    fn process_locks(&mut self, pattern: &Pattern, slot: i64) {
        for p in 0..N_LOCK_PARAMS {
            if self.revert_ticks[p] > 0 {
                self.revert_ticks[p] -= 1;
                if self.revert_ticks[p] == 0 {
                    self.override_val[p] = None;
                }
            }
        }
        let grid = pattern.grid();
        let total = grid.total_subs() as i64;
        let (b, k) = grid.sub_of_index(slot.rem_euclid(total) as u32);
        for hit in pattern.hits() {
            if hit.beat as usize != b || hit.sub as u32 != k {
                continue;
            }
            for p in 0..N_LOCK_PARAMS {
                if let Some(lock) = hit.locks[p] {
                    self.override_val[p] = Some(lock.value);
                    self.revert_ticks[p] = match lock.termination {
                        Termination::Revert { n } => n.max(1) as u32,
                        Termination::Latch => 0,
                    };
                }
            }
        }
    }

    #[inline]
    fn next_unit(&mut self) -> f32 {
        // xorshift32 → [0, 1)
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    #[inline]
    fn fires(&mut self, probability: f32) -> bool {
        // The extremes short-circuit *without* consuming the stream, so a lane of
        // p=1 (or p=0) trigs is bit-reproducible whatever else is on the lane.
        if probability >= 1.0 {
            true
        } else if probability <= 0.0 {
            false
        } else {
            self.next_unit() < probability
        }
    }

    /// Schedule this lane's hits for a block of `frames` samples starting at
    /// `beat0`, at `bps` beats-per-sample, appending to `out`. `out` is cleared
    /// first. When `!playing`, emits nothing and parks the lane (no advance).
    /// Allocation-free as long as `out` has spare capacity.
    pub fn schedule(
        &mut self,
        pattern: &Pattern,
        beat0: f64,
        bps: f64,
        frames: usize,
        playing: bool,
        out: &mut Vec<TrigEvent>,
    ) {
        out.clear();
        // `!(bps > 0.0)` rather than `bps <= 0.0`, and `beat0` checked at all: a
        // host is free to report a NaN or infinite tempo, and both comparisons
        // below are `false` against a NaN, so a poisoned clock would run the
        // resolve loops without ever reaching their exit test.
        #[allow(clippy::neg_cmp_op_on_partial_ord, reason = "the negation is the NaN guard")]
        if !playing || !(bps > 0.0) || !bps.is_finite() || !beat0.is_finite() || frames == 0 {
            // Park: a fresh phase will be (re)established when playback resumes.
            self.reset();
            return;
        }

        let grid = pattern.grid();
        // The lane's mean slot. Only the jump tolerance is measured in it — it is
        // the scale at which "did the transport move, or merely advance?" is a
        // sensible question. Every position below comes from the real geometry.
        let nominal = grid.len_beats() / grid.total_subs() as f64;
        let beat_end = beat0 + frames as f64 * bps;

        // Transport-jump resync: if the block didn't continue where the last one
        // ended, drop the window (and with it every fire time resolved for the
        // timeline we just left) and re-anchor both cursors.
        if (beat0 - self.expected_beat).abs() > nominal * 0.5 {
            self.window.clear();
            // Rewind both cursors rather than anchoring them at the jump target:
            // the per-block clamp below already skips whatever the transport has
            // run past, and an anchor computed from `beat0` would additionally
            // skip a position sitting *exactly* on it.
            self.next_lock_beat = f64::NEG_INFINITY;
            self.next_trig_beat = f64::NEG_INFINITY;
            // A seek discards in-flight p-lock holds — re-establish cold.
            self.override_val = [None; N_LOCK_PARAMS];
            self.revert_ticks = [0; N_LOCK_PARAMS];
        }
        self.expected_beat = beat_end;

        // Re-anchor either cursor whose index no longer names the position it was
        // committed at — the pattern was edited under us (see `next_trig_beat`).
        // The comparison is exact because it is the same computation on unchanged
        // data; anything else means the numbering moved.
        if grid.slot_pos(self.next_lock_index) != self.next_lock_beat {
            self.next_lock_index = first_slot_at_or_after(grid, self.next_lock_beat);
        }
        if pattern.fire_beat_at(self.next_trig_index) != self.next_trig_beat {
            self.next_trig_index = pattern.hit_at_or_after(self.next_trig_beat);
        }

        // Both cursors then clamp per block to the first position at or after this
        // block's start, so a lane that fell behind the transport skips forward
        // rather than replaying stale positions.
        //
        // The clamp is **per block, never committed to the cursor**: a cursor
        // records positions actually resolved, nothing more. Folding the clamp
        // into it would strand the cursor ahead of the timeline whenever a block
        // crosses nothing at all, and the clamp does not only grow — it drops on a
        // backwards seek smaller than the resync tolerance above, and would then
        // silently skip the positions in between, losing their trigs and — for a
        // `Latch` — their p-lock for good.

        // 1. p-locks advance per *crossed* subdivision slot, in slot order,
        //    independent of trigs (ADR 0004 §3) — so they are on their own cursor
        //    and their own horizon, the block end.
        let mut slot = self.next_lock_index.max(first_slot_at_or_after(grid, beat0));
        while grid.slot_pos(slot) < beat_end {
            self.process_locks(pattern, slot);
            slot += 1;
            self.next_lock_index = slot;
            self.next_lock_beat = grid.slot_pos(slot);
        }

        // 2. Fire times carried in from previous blocks that are due now.
        self.window.emit_due(beat0, beat_end, bps, frames, out);

        // 3. Resolve hits into the window out to the lookahead horizon, emitting
        //    each hit's due fire times as it resolves so `out` stays in resolve
        //    order (the sort below only has to fix frame ties between a retrig and
        //    a later hit).
        //
        //    One horizon for the whole block, not one per hit: hits sharing a fire
        //    time must resolve together, or the cursor would come to rest on a
        //    time it has already half-resolved and re-roll the earlier ones.
        let horizon = beat_end + EARLY_SLOTS * nominal;
        if !pattern.is_empty() {
            let mut index = self.next_trig_index.max(pattern.hit_at_or_after(beat0));
            while let Some(hit) = pattern.hit_at(index) {
                let fire = pattern.fire_beat_at(index);
                if fire >= horizon {
                    break;
                }
                if self.fires(hit.probability) {
                    let modulation = trig_mod(pattern, &hit, fire, index);
                    if hit.retrig.is_retrig() {
                        let span = pattern.retrig_span(index, hit.retrig.m);
                        self.expand_retrig(&hit, fire, span, modulation);
                    } else {
                        self.window.push(Pending {
                            beat: fire,
                            note: hit.note,
                            velocity: hit.velocity,
                            modulation,
                            from_retrig: false,
                        });
                    }
                    self.window.emit_due(beat0, beat_end, bps, frames, out);
                }
                index += 1;
                // Committed inside the loop, for the reason given above.
                self.next_trig_index = index;
                self.next_trig_beat = pattern.fire_beat_at(index);
            }
        }

        // Keep hits frame-ordered for the sub-span renderer.
        out.sort_unstable_by_key(|h| h.frame);
    }

    /// Expand a retrig macro into `n` fire times in the window, anchored at the
    /// hit's actual fire time `origin` (ADR 0007 §9: micro-timing offsets the
    /// retrig *window origin*; the n-over-m subdivision runs relative to it).
    /// `span` is the width of the macro's `m` subdivision slots, measured on the
    /// real grid. Replaces the previous retrig's pending tail — a lane has one
    /// live retrig.
    ///
    /// Every sub-hit carries the parent hit's `modulation` unchanged: a retrig is one
    /// hit's expansion, so its colour and its in-slot position are the *hit's*
    /// properties, not each sub-hit's. Only velocity ramps.
    fn expand_retrig(&mut self, hit: &Hit, origin: f64, span: f64, modulation: TrigMod) {
        self.window.drop_retrig_tail();
        let n = hit.retrig.n as u32;
        for j in 0..n {
            // Timing walks `j/n` through the curve (so hit 0 sits at the window
            // start and hit n-1 short of its end); velocity ramps over `j/(n-1)`
            // so it reaches `vel_end` exactly on the last hit. The different
            // denominators are deliberate.
            //
            // `is_retrig()` gates `n >= 2`, but `Retrig.n` is public data and the
            // guard costs nothing — without it `n == 1` divides by zero and
            // velocity comes out NaN.
            let velocity = if n <= 1 {
                hit.velocity
            } else {
                let f = j as f32 / (n - 1) as f32;
                (hit.velocity + (hit.retrig.vel_end - hit.velocity) * f).clamp(0.0, 1.0)
            };
            let u = j as f64 / n as f64;
            self.window.push(Pending {
                beat: origin + hit.retrig.curve.position(u) * span,
                note: hit.note,
                velocity,
                modulation,
                from_retrig: true,
            });
        }
    }
}

/// The first global subdivision slot at or **after** `t`. `Grid::slot_at` floors;
/// the p-lock cursor wants the ceiling, so a slot the transport is already past is
/// not replayed.
#[inline]
fn first_slot_at_or_after(grid: &Grid, t: f64) -> i64 {
    let g = grid.slot_at(t);
    if grid.slot_pos(g) >= t { g } else { g + 1 }
}

#[inline]
fn frame_of(beat: f64, beat0: f64, bps: f64, frames: usize) -> usize {
    (((beat - beat0) / bps).round() as i64).clamp(0, frames as i64) as usize
}

/// Push a trig, dropping it if `out` is at capacity (never reallocates on the
/// audio thread — a dropped trig is preferable to an allocation).
#[inline]
fn push_hit(out: &mut Vec<TrigEvent>, frame: usize, note: f32, velocity: f32, modulation: TrigMod) {
    if out.len() < out.capacity() {
        out.push(TrigEvent {
            frame,
            note,
            velocity,
            modulation,
        });
    }
}

/// The modulation a resolved hit carries to its trig (ADR 0007 §7): its colour as a
/// macro vector, and its **lateness** — where it landed inside its own subdivision slot.
/// Pure and `Copy`-only, so allocation-free; runs once per resolved hit, never per
/// sample.
///
/// Lateness is the *resolved* fraction
///
/// ```text
/// (fire_beat(i) - sub_pos(beat, sub)) / slot_span(i)
/// ```
///
/// and deliberately **not** the stored `hit.f`. The two are different quantities: 0348
/// stores `f` as a proportion of its slot *precisely so it scales with the slot*, which
/// makes it swing-invariant — a hit 40% into its slot reads 0.4 at every swing amount,
/// even as the slot lengthens under it and the hit moves in absolute time. The resolved
/// fraction is measured against the markers where the warp actually put them, so it is
/// lateness against the *swung* grid (the more musical modulator, ADR 0007 §7), and it
/// carries the unscaled `nudge`, which `f` does not hold at all.
fn trig_mod(pattern: &Pattern, hit: &Hit, fire: f64, index: i64) -> TrigMod {
    let macros = crate::flavour::colour_override(hit.rgb);
    let n = pattern.len() as i64;
    let span = pattern.slot_span_at(index);
    // `MIN_SLOT` makes a zero-width slot unreachable; the guard is what keeps the
    // division below from producing the NaN that a degenerate grid otherwise would.
    if n == 0 || !span.is_finite() || span <= 0.0 {
        return TrigMod { macros, lateness: 0.0 };
    }
    let grid = pattern.grid();
    // Clamped exactly as `Pattern::fire_beat` clamps, so this is measured against the
    // very marker the fire time was built from.
    let b = (hit.beat as usize).min(grid.n_beats() - 1);
    let k = (hit.sub as u32).min(grid.subs(b) - 1);
    let marker = index.div_euclid(n) as f64 * grid.len_beats() + grid.sub_pos(b, k);
    let lateness = ((fire - marker) / span) as f32;
    TrigMod {
        macros,
        lateness: if lateness.is_finite() { lateness } else { 0.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::{Lock, LockParam, Pattern, Retrig, RetrigCurve, Termination};

    const BPS: f64 = 120.0 / 60.0 / 48_000.0; // beats per sample @120/48k
    const STEP_FRAMES: usize = 6_000; // one 16th at 120/48k
    const G: usize = 0; // LockParam::Gain.index()

    /// Advance the lane by exactly one 16th (slot `k` of the default grid),
    /// returning the gain override after that slot.
    fn step(lane: &mut LaneState, pat: &Pattern, k: i64) -> Option<f32> {
        let mut hits = Vec::with_capacity(8);
        lane.schedule(pat, k as f64 * 0.25, BPS, STEP_FRAMES, true, &mut hits);
        lane.override_value(G)
    }

    /// A lane with one hit in `slot` carrying `lock` on gain.
    fn locked_at(pat: &mut Pattern, slot: usize, lock: Lock) {
        pat.set(slot, 36.0, 1.0);
        let i = pat.hit_index_at_slot(slot as u32).unwrap();
        pat.set_lock(i, LockParam::Gain, lock);
    }

    #[test]
    fn revert_n1_holds_one_tick() {
        let mut pat = Pattern::default();
        locked_at(&mut pat, 2, Lock { value: 0.3, termination: Termination::Revert { n: 1 } });
        let mut lane = LaneState::new(0);
        assert_eq!(step(&mut lane, &pat, 0), None);
        assert_eq!(step(&mut lane, &pat, 1), None);
        assert_eq!(step(&mut lane, &pat, 2), Some(0.3), "fires at its slot");
        assert_eq!(step(&mut lane, &pat, 3), None, "released after 1 slot");
    }

    #[test]
    fn revert_n2_holds_then_releases() {
        let mut pat = Pattern::default();
        locked_at(&mut pat, 2, Lock { value: 0.3, termination: Termination::Revert { n: 2 } });
        let mut lane = LaneState::new(0);
        for k in 0..2 {
            assert_eq!(step(&mut lane, &pat, k), None);
        }
        assert_eq!(step(&mut lane, &pat, 2), Some(0.3));
        assert_eq!(step(&mut lane, &pat, 3), Some(0.3), "still held at slot 2");
        assert_eq!(step(&mut lane, &pat, 4), None, "released after N=2 slots");
    }

    /// AC: `Revert { n }` counts **subdivision slots**, on a lane whose sub-count
    /// varies per beat — so the hold is a count of grid positions, not a duration.
    #[test]
    fn revert_counts_subdivision_slots_on_a_varying_sub_count() {
        // Beat 0 in three, the rest in four. A four-slot revert therefore spans
        // the whole tuplet beat plus one 16th, and the slots it counts are of two
        // different widths.
        let mut pat = Pattern::default();
        pat.edit_grid(|g| g.set_beat_subs(0, Some(3)));
        locked_at(&mut pat, 0, Lock { value: 0.4, termination: Termination::Revert { n: 4 } });
        assert_eq!(pat.total_subs(), 3 + 4 + 4 + 4);

        let grid = *pat.grid();
        assert_ne!(grid.slot_span(0), grid.slot_span(3), "the slots differ in width");

        // One block per slot, contiguous, so exactly one slot is crossed each time.
        let mut lane = LaneState::new(0);
        let mut hits = Vec::with_capacity(8);
        let mut held = Vec::new();
        for g in 0..6i64 {
            let (b0, b1) = (grid.slot_pos(g), grid.slot_pos(g + 1));
            let frames = ((b1 - b0) / BPS).round() as usize;
            lane.schedule(&pat, b0, BPS, frames, true, &mut hits);
            held.push(lane.override_value(G));
        }
        assert_eq!(
            held,
            vec![Some(0.4), Some(0.4), Some(0.4), Some(0.4), None, None],
            "held for four slots, not for four of anything else"
        );
    }

    #[test]
    fn latch_holds_until_next_lock_and_across_wrap() {
        // Short loop (one beat, four subs) so we cross the wrap quickly.
        let mut pat = Pattern::default();
        pat.set_grid_beats(1);
        locked_at(&mut pat, 1, Lock { value: 0.6, termination: Termination::Latch });
        let mut lane = LaneState::new(0);
        assert_eq!(step(&mut lane, &pat, 0), None);
        assert_eq!(step(&mut lane, &pat, 1), Some(0.6));
        assert_eq!(step(&mut lane, &pat, 2), Some(0.6), "latched");
        assert_eq!(step(&mut lane, &pat, 3), Some(0.6));
        // Loop wrap (slot 4 == lane slot 0): latch persists.
        assert_eq!(step(&mut lane, &pat, 4), Some(0.6), "persists across wrap");
        assert_eq!(step(&mut lane, &pat, 5), Some(0.6));
    }

    #[test]
    fn new_lock_preempts_in_flight_hold() {
        let mut pat = Pattern::default();
        locked_at(&mut pat, 1, Lock { value: 0.2, termination: Termination::Revert { n: 8 } });
        locked_at(&mut pat, 2, Lock { value: 0.9, termination: Termination::Latch });
        let mut lane = LaneState::new(0);
        assert_eq!(step(&mut lane, &pat, 0), None);
        assert_eq!(step(&mut lane, &pat, 1), Some(0.2), "revert begins");
        assert_eq!(step(&mut lane, &pat, 2), Some(0.9), "preempted by latch");
        assert_eq!(step(&mut lane, &pat, 3), Some(0.9), "held (not the old revert)");
    }

    #[test]
    fn transport_jump_clears_holds() {
        let mut pat = Pattern::default();
        locked_at(&mut pat, 1, Lock { value: 0.5, termination: Termination::Latch });
        let mut lane = LaneState::new(0);
        step(&mut lane, &pat, 0);
        assert_eq!(step(&mut lane, &pat, 1), Some(0.5));
        // Jump far away (no lock there): the latch is dropped, re-established cold.
        let mut hits = Vec::with_capacity(8);
        lane.schedule(&pat, 40.0, BPS, STEP_FRAMES, true, &mut hits);
        assert_eq!(lane.override_value(G), None, "seek clears in-flight holds");
    }

    // ── Lookahead window (0346) ──────────────────────────────────────────────

    /// Drive one lane over `total` frames, cutting blocks at the sample offsets
    /// in `cuts`. Returns the *absolute* hit frames and the final PRNG state —
    /// the PRNG is the draw counter, so it pins how many probability rolls the
    /// chunking cost.
    fn drive(pat: &Pattern, seed: usize, total: usize, cuts: &[usize]) -> (Vec<usize>, u32) {
        let mut lane = LaneState::new(seed);
        let mut hits = Vec::with_capacity(64);
        let mut absolute = Vec::new();
        let mut p = 0usize;
        for &bound in cuts.iter().chain(std::iter::once(&total)) {
            if bound <= p {
                continue;
            }
            lane.schedule(pat, p as f64 * BPS, BPS, bound - p, true, &mut hits);
            absolute.extend(hits.iter().map(|h| p + h.frame));
            p = bound;
        }
        (absolute, lane.rng)
    }

    /// A trig whose slot straddles a block boundary must be rolled **once**: the
    /// resolve cursor, not the block walk, decides when a hit is evaluated.
    #[test]
    fn probability_is_drawn_once_per_trig_across_block_splits() {
        let mut pat = Pattern::default();
        for s in 0..8 {
            pat.set(s, 36.0, 1.0);
            pat.set_probability(s, 0.5);
        }
        let total = 8 * STEP_FRAMES;

        // Reference: the whole span resolved in a single block.
        let (want_hits, want_rng) = drive(&pat, 5, total, &[]);
        assert!(!want_hits.is_empty(), "the reference must actually fire");

        for cuts in [
            vec![STEP_FRAMES / 2],           // mid-slot
            vec![STEP_FRAMES],               // exactly on a slot boundary
            vec![STEP_FRAMES - 1],           // one sample early
            vec![STEP_FRAMES + 1],           // one sample late
            vec![1, STEP_FRAMES / 3, STEP_FRAMES, 2 * STEP_FRAMES + 7, 5 * STEP_FRAMES - 1],
        ] {
            let (got_hits, got_rng) = drive(&pat, 5, total, &cuts);
            assert_eq!(got_hits, want_hits, "cuts {cuts:?}: block splits moved hits");
            assert_eq!(got_rng, want_rng, "cuts {cuts:?}: probability was re-rolled");
        }
    }

    /// A seek drops resolved-but-unfired hits: the retrig tail belongs to the
    /// timeline the transport just left.
    #[test]
    fn transport_jump_drops_the_lookahead_window() {
        let mut pat = Pattern::default();
        pat.set(0, 36.0, 1.0);
        pat.set_retrig(0, Retrig { n: 4, m: 2, curve: RetrigCurve::Even, vel_end: 1.0 });
        let mut lane = LaneState::new(1);
        let mut hits = Vec::with_capacity(64);

        // Half a slot: only the retrig's first of four hits is due.
        lane.schedule(&pat, 0.0, BPS, STEP_FRAMES / 2, true, &mut hits);
        assert_eq!(hits.len(), 1, "only the window's first hit lands in this block");

        // Seek onto an empty slot: the carried tail must not surface there.
        lane.schedule(&pat, 40.25, BPS, STEP_FRAMES, true, &mut hits);
        assert!(hits.is_empty(), "seek must drop the in-flight retrig window");
    }

    /// The window carries a retrig across arbitrarily many blocks, unchanged by
    /// how the block boundaries fall.
    #[test]
    fn retrig_window_is_block_size_invariant() {
        let mut pat = Pattern::default();
        pat.set(0, 36.0, 1.0);
        pat.set_retrig(0, Retrig { n: 4, m: 2, curve: RetrigCurve::Even, vel_end: 1.0 });
        let total = 2 * STEP_FRAMES;
        // 4 hits evenly over 2 slots (span 12000 frames) → 0, 3000, 6000, 9000.
        let want = vec![0, 3_000, 6_000, 9_000];
        for cuts in [
            vec![],
            vec![512, 1_024, 3_000, 5_999, 9_001],
            vec![2_999, 3_001, 8_999],
        ] {
            let (got, _) = drive(&pat, 2, total, &cuts);
            assert_eq!(got, want, "cuts {cuts:?}");
        }
    }

    /// A lane has **one** live retrig: a new one replaces the pending tail of
    /// the last, exactly as the pre-0346 single in-flight `rt_*` slot did.
    ///
    /// Since the horizon reaches past the block end (`EARLY_SLOTS`), the
    /// replacement happens when the new retrig *resolves* rather than when its
    /// first hit is emitted — so the old window's entry at the same frame is now
    /// dropped instead of doubling the new retrig's first hit. A cleaner answer,
    /// and the reason this expectation is one hit shorter than 0346's.
    #[test]
    fn a_new_retrig_replaces_the_pending_tail_of_the_last() {
        let cuts: Vec<usize> = (1..4).map(|k| k * STEP_FRAMES).collect();
        let total = 4 * STEP_FRAMES;

        // A four-slot retrig window, one hit per slot.
        let mut solo = Pattern::default();
        solo.set(0, 36.0, 1.0);
        solo.set_retrig(0, Retrig { n: 4, m: 4, curve: RetrigCurve::Even, vel_end: 1.0 });
        let (got, _) = drive(&solo, 4, total, &cuts);
        assert_eq!(got, vec![0, 6_000, 12_000, 18_000], "4 hits over 4 slots");

        // A second retrig lands inside that window, at slot 2.
        let mut preempted = solo;
        preempted.set(2, 40.0, 1.0);
        preempted.set_retrig(2, Retrig { n: 2, m: 1, curve: RetrigCurve::Even, vel_end: 1.0 });
        let (got, _) = drive(&preempted, 4, total, &cuts);
        assert_eq!(
            got,
            vec![0, 6_000, 12_000, 15_000],
            "slot 2's retrig takes over the window from its own origin on"
        );
    }

    /// A block that crosses no grid position must leave the cursors alone. They
    /// clamp to the first position at or after the block start, and that floor can
    /// *drop* — here by a backwards seek too small to trip the resync, and equally
    /// by a live grid edit. Committing the clamp would strand the cursors past the
    /// skipped position, losing its trig and its p-lock for good.
    #[test]
    fn a_block_crossing_no_position_does_not_advance_the_cursors() {
        // Five beats of two subs: slots half a beat wide, exactly the old
        // `len: 5, step_beats: 0.5` lane, expressed as geometry.
        let mut pat = Pattern::default();
        pat.set_grid_beats(5);
        pat.set_grid_subs(2);
        locked_at(&mut pat, 1, Lock { value: 0.25, termination: Termination::Latch });
        let mut lane = LaneState::new(0);
        let mut hits = Vec::with_capacity(8);

        // Slot 171 spans beats 85.5..86.0 and holds the lane's slot 1 (171 % 10).
        // This block sits inside it and crosses no position at all.
        assert_eq!(pat.grid().slot_pos(171), 85.5);
        lane.schedule(&pat, 85.6, BPS, 1_000, true, &mut hits);
        assert!(hits.is_empty(), "no position crossed");

        // Seek 0.1 beats back — inside the half-slot resync tolerance, so the lane
        // does *not* re-anchor — onto a block that does cross 171.
        lane.schedule(&pat, 85.5, BPS, 1_000, true, &mut hits);
        assert_eq!(hits.len(), 1, "position 171 must still be resolvable");
        assert_eq!(
            lane.override_value(G),
            Some(0.25),
            "position 171's latch must not have been skipped"
        );
    }

    /// Both cursors are indices into a numbering the *pattern* defines, and the
    /// pattern is edited from the audio thread. An edit that changes `len()` or
    /// the grid's slot count rewrites what an already-committed index means — by a
    /// margin proportional to how long the lane has been playing, so a cursor
    /// carried across such an edit strands the lane silent (and its p-locks
    /// frozen) for minutes.
    #[test]
    fn a_live_edit_does_not_strand_the_cursors() {
        let mut pat = Pattern::default();
        for s in 0..16 {
            pat.set(s, 36.0, 1.0);
        }
        // A revert spike on the downbeat, so the p-lock cursor has something to show.
        pat.set_lock(0, LockParam::Gain, Lock { value: 0.5, termination: Termination::Revert { n: 1 } });
        let mut lane = LaneState::new(0);
        let mut hits = Vec::with_capacity(64);
        // A hundred beats in, so a stranded cursor is stranded by a wide margin.
        for k in 0..400i64 {
            lane.schedule(&pat, k as f64 * 0.25, BPS, STEP_FRAMES, true, &mut hits);
        }

        // Halve the lane's sub-count (16 slots → 8) and delete a hit (16 → 15):
        // the divisor behind *each* of the two global indices moves at once.
        pat.set_grid_subs(2);
        pat.remove(1);

        let (mut fired, mut locked) = (0usize, 0usize);
        for k in 400..416i64 {
            lane.schedule(&pat, k as f64 * 0.25, BPS, STEP_FRAMES, true, &mut hits);
            fired += hits.len();
            locked += lane.override_value(G).is_some() as usize;
        }
        assert!(fired > 0, "the lane went silent after a live edit");
        assert!(locked > 0, "p-locks stopped resolving after a live edit");
    }

    /// A host reporting a NaN or infinite tempo must park the lane, not spin the
    /// resolve loops — every float comparison in them is `false` against a NaN,
    /// including the ones that end them.
    #[test]
    fn a_non_finite_clock_parks_the_lane() {
        let mut pat = Pattern::default();
        pat.set(0, 36.0, 1.0);
        let mut lane = LaneState::new(0);
        let mut hits = Vec::with_capacity(8);
        for (beat0, bps) in [
            (0.0, f64::NAN),
            (0.0, f64::INFINITY),
            (f64::NAN, BPS),
            (f64::INFINITY, BPS),
        ] {
            lane.schedule(&pat, beat0, bps, STEP_FRAMES, true, &mut hits);
            assert!(hits.is_empty(), "beat0={beat0} bps={bps}");
        }
        // …and a sane clock afterwards still works.
        lane.schedule(&pat, 0.0, BPS, STEP_FRAMES, true, &mut hits);
        assert_eq!(hits.len(), 1);
    }

    /// A hit nudged early fires **before** its own marker, and before the block
    /// that marker falls in — the whole reason the horizon reaches past the block
    /// end (ADR 0007 §9).
    #[test]
    fn an_early_nudged_hit_fires_before_its_marker() {
        use crate::sequencer::MAX_NUDGE_TICKS;
        let mut pat = Pattern::default();
        pat.set(4, 36.0, 1.0); // beat 1, dead on
        pat.set_offset(0, 0.0, -MAX_NUDGE_TICKS);
        // ½ MIN_SLOT is 1/128 beat = 187.5 frames at 120 bpm / 48 kHz, and the
        // frame it lands on is that rounded — sub-sample accuracy is not on offer.
        let (got, _) = drive(&pat, 0, 8 * STEP_FRAMES, &[]);
        assert_eq!(got, vec![4 * STEP_FRAMES - 187]);
        // …and the block split that would strand it makes no difference.
        let (split, _) = drive(&pat, 0, 8 * STEP_FRAMES, &[4 * STEP_FRAMES - 500]);
        assert_eq!(split, got, "the early hit must not be lost at a block boundary");
    }
}
