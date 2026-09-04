//! Per-track sequencer state and the per-block hit scheduler (ADR 0001 §2).
//!
//! Each track resolves its [`Pattern`] against the host beat clock on its **own**
//! lane-local tick, so lanes with different lengths/divisors phase (polymeter).
//!
//! **Scheduling model (0346).** Fire times are points on a *continuous* per-lane
//! beat timeline, not step indices: each block advances a bounded **lookahead
//! window** over that timeline, resolving grid positions into fire times, emits
//! everything in the window landing in `[beat0, beat_end)`, and carries the
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
//!   times in the window when its position resolves, and the window carries them.
//! - p-locks resolve on a **separate cursor** over the same timeline
//!   ([`LaneState::next_lock_index`]) — per *crossed* grid position, independent
//!   of trigs and of the lookahead horizon (ADR 0004 §3: independent axes).
//! - A transport jump drops the window along with the in-flight state it replaces.
//!
//! Output is a flat list of sample-accurate [`Hit`]s for the block; the engine
//! slices the track's render at those offsets.

use crate::sequencer::{N_LOCK_PARAMS, Pattern, Termination};

/// A scheduled trig within a block: a sample offset + note + velocity.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Hit {
    pub frame: usize,
    pub note: f32,
    pub velocity: f32,
}

// ── Lookahead window sizing (ADR 0007 §9) ─────────────────────────────────────
//
// The window is bounded by the **monotonic-fire-order** invariant: a hit cannot
// leave its own slot far enough to reorder past a neighbour. The in-slot
// fraction `f ∈ [0, 1)` keeps a hit inside its slot by construction, and `nudge`
// — the only term that can move a hit *backwards* — is clamped to ±½ `MIN_SLOT`.
// So a hit's fire time always lies in `[-½, +1½)` slots of its own grid
// position. That is what makes the window const-sized, hence preallocated and
// alloc-free in `schedule` (and so on the audio thread).

/// Slots a hit's fire time may sit **before** its own grid position — the
/// ±½ `MIN_SLOT` `nudge` clamp (ADR 0007 §9). This is the ceiling on how far
/// past the block end positions must be resolved.
const MAX_EARLY_SLOTS: f64 = 0.5;

/// The early offset actually in play on this build's grid. Today's `[Step; 16]`
/// carries no per-hit offset at all — every hit fires exactly on its position —
/// so nothing can be early and the horizon is the block end. 0348 replaces this
/// with the lane's real bound; the ceiling above (and so the window's size) does
/// not move, which is the point of sizing from the invariant rather than from
/// today's zero.
const EARLY_SLOTS: f64 = 0.0;
const _: () = assert!(EARLY_SLOTS <= MAX_EARLY_SLOTS);

/// Slots a hit's fire time may sit **after** its own grid position: the in-slot
/// fraction `f ∈ [0, 1)` that 0348 adds, plus the ½-slot late `nudge`.
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
/// pending — a new one replaces the previous one's tail) plus one plain hit per
/// lookahead position. No legal pattern can overflow it; a push beyond capacity
/// drops the hit rather than allocate, matching [`push_hit`].
const WINDOW_CAPACITY: usize = MAX_HITS_PER_POSITION + LOOKAHEAD_POSITIONS;

/// A fire time resolved onto the lane's continuous timeline but not yet emitted.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Pending {
    /// Absolute position on the host beat clock — a time, not a grid index.
    beat: f64,
    note: f32,
    velocity: f32,
    /// Came from a retrig expansion. A new retrig replaces the pending tail of
    /// the previous one (one live retrig per lane, as before 0346); plain hits
    /// are untouched by that.
    from_retrig: bool,
}

const EMPTY_PENDING: Pending = Pending {
    beat: 0.0,
    note: 0.0,
    velocity: 0.0,
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
        out: &mut Vec<Hit>,
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
                push_hit(out, frame, entry.note, entry.velocity);
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
    /// Next lane position whose trigs have not been resolved into the window.
    /// A position resolves exactly once, which is what makes probability draw
    /// once per primary trig even when its slot straddles a block boundary.
    next_trig_index: i64,
    /// Next lane position whose p-locks have not been applied. Tracks *crossed*
    /// positions, so it lags the trig cursor by the lookahead horizon (they
    /// coincide while nothing fires early — see [`EARLY_SLOTS`]).
    next_lock_index: i64,
    /// Fire times resolved onto the timeline but not yet emitted.
    window: Window,

    // ── p-lock resolver (per lockable param) ──
    /// Active override value, or `None` when the param falls back to base.
    override_val: [Option<f32>; N_LOCK_PARAMS],
    /// Lane-local ticks left on a `Revert` hold (`0` = not reverting; a latched
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
            next_trig_index: i64::MIN,
            next_lock_index: i64::MIN,
            window: Window::new(),
            override_val: [None; N_LOCK_PARAMS],
            revert_ticks: [0; N_LOCK_PARAMS],
        }
    }

    /// Reset transport-derived phase + the lookahead window + p-lock overrides
    /// (transport stop / engine reset). The PRNG stream is left running.
    pub fn reset(&mut self) {
        self.expected_beat = f64::NEG_INFINITY;
        self.next_trig_index = i64::MIN;
        self.next_lock_index = i64::MIN;
        self.window.clear();
        self.override_val = [None; N_LOCK_PARAMS];
        self.revert_ticks = [0; N_LOCK_PARAMS];
    }

    /// The active p-lock override for `param_index`, or `None` to use base.
    #[inline]
    pub fn override_value(&self, param_index: usize) -> Option<f32> {
        self.override_val[param_index]
    }

    /// Advance + apply p-locks for one crossed lane boundary at `global_index`.
    /// Existing reverts tick down first (so a lock set this boundary isn't
    /// decremented this boundary); then this step's locks apply, superseding any
    /// in-flight hold (preemption, no queue).
    fn process_locks(&mut self, pattern: &Pattern, global_index: i64) {
        for p in 0..N_LOCK_PARAMS {
            if self.revert_ticks[p] > 0 {
                self.revert_ticks[p] -= 1;
                if self.revert_ticks[p] == 0 {
                    self.override_val[p] = None;
                }
            }
        }
        for p in 0..N_LOCK_PARAMS {
            if let Some(lock) = pattern.lock_at(global_index, p) {
                self.override_val[p] = Some(lock.value);
                self.revert_ticks[p] = match lock.termination {
                    Termination::Revert { n } => n.max(1) as u32,
                    Termination::Latch => 0,
                };
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
        out: &mut Vec<Hit>,
    ) {
        out.clear();
        if !playing || bps <= 0.0 || frames == 0 {
            // Park: a fresh phase will be (re)established when playback resumes.
            self.reset();
            return;
        }

        let sb = pattern.step_beats.max(1e-9);
        let beat_end = beat0 + frames as f64 * bps;

        // Transport-jump resync: if the block didn't continue where the last one
        // ended, drop the window (and with it every fire time resolved for the
        // timeline we just left) and re-anchor both cursors.
        if (beat0 - self.expected_beat).abs() > sb * 0.5 {
            self.window.clear();
            let anchor = (beat0 / sb).floor() as i64;
            self.next_trig_index = anchor;
            self.next_lock_index = anchor;
            // A seek discards in-flight p-lock holds — re-establish cold.
            self.override_val = [None; N_LOCK_PARAMS];
            self.revert_ticks = [0; N_LOCK_PARAMS];
        }
        self.expected_beat = beat_end;

        // The first grid position at or after this block's start. Both cursors
        // clamp to it so a lane that fell behind the transport skips forward
        // rather than replaying stale positions.
        //
        // The clamp is **per block, never committed to the cursor**: a cursor
        // records positions actually resolved, nothing more. Folding `first`
        // into it would strand the cursor ahead of the timeline whenever a block
        // crosses no position at all, and `first` does not only grow — it drops
        // when `step_beats` is edited live (a wider slot ⇒ lower index), and on
        // a backwards seek smaller than the resync tolerance above. Either one
        // would then silently skip the positions in between, losing their trigs
        // and — for a `Latch` — their p-lock for good.
        let first = (beat0 / sb).ceil() as i64;

        // 1. p-locks advance per *crossed* grid position, in position order,
        //    independent of trigs (ADR 0004 §3) — so they are on their own
        //    cursor and their own horizon, the block end.
        let mut lock_index = self.next_lock_index.max(first);
        while (lock_index as f64) * sb < beat_end {
            self.process_locks(pattern, lock_index);
            lock_index += 1;
            self.next_lock_index = lock_index;
        }

        // 2. Fire times carried in from previous blocks that are due now.
        self.window.emit_due(beat0, beat_end, bps, frames, out);

        // 3. Resolve grid positions into the window out to the lookahead
        //    horizon, emitting each position's due hits as it resolves so `out`
        //    stays in resolve order (the sort below only has to fix frame ties
        //    between a retrig and a later position).
        let horizon = beat_end + EARLY_SLOTS * sb;
        let mut trig_index = self.next_trig_index.max(first);
        while (trig_index as f64) * sb < horizon {
            let step = pattern.step_at(trig_index);
            if step.active && self.fires(step.probability) {
                let origin = trig_index as f64 * sb;
                if step.retrig.is_retrig() {
                    self.expand_retrig(&step, origin, sb);
                } else {
                    self.window.push(Pending {
                        beat: origin,
                        note: step.note,
                        velocity: step.velocity,
                        from_retrig: false,
                    });
                }
                self.window.emit_due(beat0, beat_end, bps, frames, out);
            }
            trig_index += 1;
            // Committed inside the loop, for the reason given at `first` above.
            self.next_trig_index = trig_index;
        }

        // Keep hits frame-ordered for the sub-span renderer.
        out.sort_unstable_by_key(|h| h.frame);
    }

    /// Expand a retrig macro into `n` fire times in the window, anchored at the
    /// trig's actual fire time `origin` (ADR 0007 §9: micro-timing offsets the
    /// retrig *window origin*; the n-over-m subdivision runs relative to it).
    /// Replaces the previous retrig's pending tail — a lane has one live retrig.
    fn expand_retrig(&mut self, step: &crate::sequencer::Step, origin: f64, sb: f64) {
        self.window.drop_retrig_tail();
        let n = step.retrig.n as u32;
        let span = step.retrig.m as f64 * sb;
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
                step.velocity
            } else {
                let f = j as f32 / (n - 1) as f32;
                (step.velocity + (step.retrig.vel_end - step.velocity) * f).clamp(0.0, 1.0)
            };
            let u = j as f64 / n as f64;
            self.window.push(Pending {
                beat: origin + step.retrig.curve.position(u) * span,
                note: step.note,
                velocity,
                from_retrig: true,
            });
        }
    }
}

#[inline]
fn frame_of(beat: f64, beat0: f64, bps: f64, frames: usize) -> usize {
    (((beat - beat0) / bps).round() as i64).clamp(0, frames as i64) as usize
}

/// Push a hit, dropping it if `out` is at capacity (never reallocates on the
/// audio thread — a dropped trig is preferable to an allocation).
#[inline]
fn push_hit(out: &mut Vec<Hit>, frame: usize, note: f32, velocity: f32) {
    if out.len() < out.capacity() {
        out.push(Hit {
            frame,
            note,
            velocity,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::{Lock, LockParam, Pattern, Retrig, RetrigCurve, Termination};

    const BPS: f64 = 120.0 / 60.0 / 48_000.0; // beats per sample @120/48k
    const STEP_FRAMES: usize = 6_000; // one 16th at 120/48k
    const G: usize = 0; // LockParam::Gain.index()

    /// Advance the lane by exactly one step (boundary `k`), returning the gain
    /// override after that step.
    fn step(lane: &mut LaneState, pat: &Pattern, k: i64) -> Option<f32> {
        let mut hits = Vec::with_capacity(8);
        lane.schedule(pat, k as f64 * 0.25, BPS, STEP_FRAMES, true, &mut hits);
        lane.override_value(G)
    }

    #[test]
    fn revert_n1_holds_one_tick() {
        let mut pat = Pattern::default();
        pat.set_lock(2, LockParam::Gain, Lock { value: 0.3, termination: Termination::Revert { n: 1 } });
        let mut lane = LaneState::new(0);
        assert_eq!(step(&mut lane, &pat, 0), None);
        assert_eq!(step(&mut lane, &pat, 1), None);
        assert_eq!(step(&mut lane, &pat, 2), Some(0.3), "fires at its step");
        assert_eq!(step(&mut lane, &pat, 3), None, "released after 1 tick");
    }

    #[test]
    fn revert_n2_holds_then_releases() {
        let mut pat = Pattern::default();
        pat.set_lock(2, LockParam::Gain, Lock { value: 0.3, termination: Termination::Revert { n: 2 } });
        let mut lane = LaneState::new(0);
        for k in 0..2 {
            assert_eq!(step(&mut lane, &pat, k), None);
        }
        assert_eq!(step(&mut lane, &pat, 2), Some(0.3));
        assert_eq!(step(&mut lane, &pat, 3), Some(0.3), "still held at tick 2");
        assert_eq!(step(&mut lane, &pat, 4), None, "released after N=2 ticks");
    }

    #[test]
    fn latch_holds_until_next_lock_and_across_wrap() {
        // Short loop so we cross the wrap quickly.
        let mut pat = Pattern {
            len: 4,
            ..Default::default()
        };
        pat.set_lock(1, LockParam::Gain, Lock { value: 0.6, termination: Termination::Latch });
        let mut lane = LaneState::new(0);
        assert_eq!(step(&mut lane, &pat, 0), None);
        assert_eq!(step(&mut lane, &pat, 1), Some(0.6));
        assert_eq!(step(&mut lane, &pat, 2), Some(0.6), "latched");
        assert_eq!(step(&mut lane, &pat, 3), Some(0.6));
        // Loop wrap (step 4 == lane index 0): latch persists.
        assert_eq!(step(&mut lane, &pat, 4), Some(0.6), "persists across wrap");
        assert_eq!(step(&mut lane, &pat, 5), Some(0.6));
    }

    #[test]
    fn new_lock_preempts_in_flight_hold() {
        let mut pat = Pattern::default();
        pat.set_lock(1, LockParam::Gain, Lock { value: 0.2, termination: Termination::Revert { n: 8 } });
        pat.set_lock(2, LockParam::Gain, Lock { value: 0.9, termination: Termination::Latch });
        let mut lane = LaneState::new(0);
        assert_eq!(step(&mut lane, &pat, 0), None);
        assert_eq!(step(&mut lane, &pat, 1), Some(0.2), "revert begins");
        assert_eq!(step(&mut lane, &pat, 2), Some(0.9), "preempted by latch");
        assert_eq!(step(&mut lane, &pat, 3), Some(0.9), "held (not the old revert)");
    }

    #[test]
    fn transport_jump_clears_holds() {
        let mut pat = Pattern::default();
        pat.set_lock(1, LockParam::Gain, Lock { value: 0.5, termination: Termination::Latch });
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
    /// resolve cursor, not the block walk, decides when a position is evaluated.
    #[test]
    fn probability_is_drawn_once_per_trig_across_block_splits() {
        let mut pat = Pattern {
            len: 2,
            ..Default::default()
        };
        pat.set_probability(0, 0.5);
        pat.set_probability(1, 0.5);
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

        // Half a step: only the retrig's first of four hits is due.
        lane.schedule(&pat, 0.0, BPS, STEP_FRAMES / 2, true, &mut hits);
        assert_eq!(hits.len(), 1, "only the window's first hit lands in this block");

        // Seek onto an empty step: the carried tail must not surface there.
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
        // 4 hits evenly over 2 steps (span 12000 frames) → 0, 3000, 6000, 9000.
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
    /// the last, exactly as the pre-0346 single in-flight `rt_*` slot did. (The
    /// hit list of 0348 is where letting them coexist gets decided; this ticket
    /// only changes the scheduler's shape.)
    #[test]
    fn a_new_retrig_replaces_the_pending_tail_of_the_last() {
        let cuts: Vec<usize> = (1..4).map(|k| k * STEP_FRAMES).collect();
        let total = 4 * STEP_FRAMES;

        // A four-step retrig window, one hit per step.
        let mut solo = Pattern::default();
        solo.set(0, 36.0, 1.0);
        solo.set_retrig(0, Retrig { n: 4, m: 4, curve: RetrigCurve::Even, vel_end: 1.0 });
        let (got, _) = drive(&solo, 4, total, &cuts);
        assert_eq!(got, vec![0, 6_000, 12_000, 18_000], "4 hits over 4 steps");

        // A second retrig lands inside that window, at step 2.
        let mut preempted = solo;
        preempted.set(2, 40.0, 1.0);
        preempted.set_retrig(2, Retrig { n: 2, m: 1, curve: RetrigCurve::Even, vel_end: 1.0 });
        let (got, _) = drive(&preempted, 4, total, &cuts);
        assert_eq!(
            got,
            vec![0, 6_000, 12_000, 12_000, 15_000],
            "step 2's retrig takes over: the first window's 18000 hit is dropped"
        );
    }

    /// A block that crosses no grid position must leave the cursors alone. They
    /// clamp to `ceil(beat0 / step_beats)` per block, and that floor can *drop*
    /// — here by a backwards seek too small to trip the resync, and equally by a
    /// live `step_beats` edit. Committing the clamp would strand the cursors
    /// past the skipped position, losing its trig and its p-lock for good.
    #[test]
    fn a_block_crossing_no_position_does_not_advance_the_cursors() {
        let mut pat = Pattern {
            len: 5,
            step_beats: 0.5,
            ..Default::default()
        };
        pat.set(1, 36.0, 1.0);
        pat.set_lock(1, LockParam::Gain, Lock { value: 0.25, termination: Termination::Latch });
        let mut lane = LaneState::new(0);
        let mut hits = Vec::with_capacity(8);

        // Slot 171 spans beats 85.5..86.0 and holds step 1 (171 % 5). This block
        // sits inside it and crosses no position at all.
        lane.schedule(&pat, 85.6, BPS, 1_000, true, &mut hits);
        assert!(hits.is_empty(), "no position crossed");

        // Seek 0.14 beats back — inside the `step_beats * 0.5` resync tolerance,
        // so the lane does *not* re-anchor — onto a block that does cross 171.
        lane.schedule(&pat, 85.5, BPS, 1_000, true, &mut hits);
        assert_eq!(hits.len(), 1, "position 171 must still be resolvable");
        assert_eq!(
            lane.override_value(G),
            Some(0.25),
            "position 171's latch must not have been skipped"
        );
    }
}
