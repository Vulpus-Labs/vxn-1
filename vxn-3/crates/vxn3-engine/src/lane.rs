//! Per-track sequencer state and the per-block hit scheduler (ADR 0001 §2).
//!
//! Each track resolves its [`Pattern`] against the host beat clock on its **own**
//! lane-local tick, so lanes with different lengths/divisors phase (polymeter).
//! Probability is drawn **once per primary trig** (so it can't be re-rolled when
//! a step straddles a block boundary), and a retrig macro is carried *in-flight*
//! across blocks. Transport jumps are detected and the lane resyncs.
//!
//! Output is a flat list of sample-accurate [`Hit`]s for the block; the engine
//! slices the track's render at those offsets.

use crate::sequencer::{N_LOCK_PARAMS, Pattern, RetrigCurve, Termination};

/// A scheduled trig within a block: a sample offset + note + velocity.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Hit {
    pub frame: usize,
    pub note: f32,
    pub velocity: f32,
}

/// Per-track sequencer state, owned by the engine (audio thread).
#[derive(Clone, Debug)]
pub struct LaneState {
    /// Per-track PRNG for probability draws (xorshift32).
    rng: u32,
    /// Beat position expected at the next block start, for jump detection.
    expected_beat: f64,
    /// Highest primary lane index already processed (so probability is drawn
    /// once per step even across block boundaries).
    last_index: i64,

    // ── in-flight retrig ──
    rt_active: bool,
    rt_base_beat: f64,
    rt_span_beats: f64,
    rt_count: u32,
    rt_next: u32,
    rt_curve: RetrigCurve,
    rt_note: f32,
    rt_vel0: f32,
    rt_vel1: f32,

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
            last_index: i64::MIN,
            rt_active: false,
            rt_base_beat: 0.0,
            rt_span_beats: 0.0,
            rt_count: 0,
            rt_next: 0,
            rt_curve: RetrigCurve::Even,
            rt_note: 0.0,
            rt_vel0: 0.0,
            rt_vel1: 0.0,
            override_val: [None; N_LOCK_PARAMS],
            revert_ticks: [0; N_LOCK_PARAMS],
        }
    }

    /// Reset transport-derived phase + in-flight retrig + p-lock overrides
    /// (transport stop / engine reset). The PRNG stream is left running.
    pub fn reset(&mut self) {
        self.expected_beat = f64::NEG_INFINITY;
        self.last_index = i64::MIN;
        self.rt_active = false;
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
        // ended, drop in-flight state and re-anchor the lane index.
        if (beat0 - self.expected_beat).abs() > sb * 0.5 {
            self.rt_active = false;
            self.last_index = (beat0 / sb).floor() as i64 - 1;
            // A seek discards in-flight p-lock holds — re-establish cold.
            self.override_val = [None; N_LOCK_PARAMS];
            self.revert_ticks = [0; N_LOCK_PARAMS];
        }
        self.expected_beat = beat_end;

        // 1. Emit any in-flight retrig hits landing in this block.
        self.emit_retrig(beat0, beat_end, bps, frames, out);

        // 2. Walk new primary boundaries in [beat0, beat_end), one eval each.
        let first = (beat0 / sb).ceil() as i64;
        let mut i = (self.last_index + 1).max(first);
        loop {
            let bb = i as f64 * sb;
            if bb >= beat_end {
                break;
            }
            self.last_index = i;
            // p-locks resolve at every crossed boundary, independent of trigs.
            self.process_locks(pattern, i);
            let step = pattern.step_at(i);
            if step.active && self.fires(step.probability) {
                if step.retrig.is_retrig() {
                    // Start a retrig window anchored at this boundary, then emit
                    // whatever of it falls inside this block.
                    self.rt_active = true;
                    self.rt_base_beat = bb;
                    self.rt_span_beats = step.retrig.m as f64 * sb;
                    self.rt_count = step.retrig.n as u32;
                    self.rt_next = 0;
                    self.rt_curve = step.retrig.curve;
                    self.rt_note = step.note;
                    self.rt_vel0 = step.velocity;
                    self.rt_vel1 = step.retrig.vel_end;
                    self.emit_retrig(beat0, beat_end, bps, frames, out);
                } else {
                    push_hit(out, frame_of(bb, beat0, bps, frames), step.note, step.velocity);
                }
            }
            i += 1;
        }

        // Keep hits frame-ordered for the sub-span renderer.
        out.sort_unstable_by_key(|h| h.frame);
    }

    /// Emit in-flight retrig hits whose time falls in `[beat0, beat_end)`.
    fn emit_retrig(
        &mut self,
        beat0: f64,
        beat_end: f64,
        bps: f64,
        frames: usize,
        out: &mut Vec<Hit>,
    ) {
        if !self.rt_active {
            return;
        }
        while self.rt_next < self.rt_count {
            let u = self.rt_next as f64 / self.rt_count as f64;
            let t = self.rt_base_beat + self.rt_curve.position(u) * self.rt_span_beats;
            if t >= beat_end {
                return; // later hits belong to a future block
            }
            if t >= beat0 - 1e-9 {
                let vel = if self.rt_count <= 1 {
                    self.rt_vel0
                } else {
                    let f = self.rt_next as f32 / (self.rt_count - 1) as f32;
                    (self.rt_vel0 + (self.rt_vel1 - self.rt_vel0) * f).clamp(0.0, 1.0)
                };
                push_hit(out, frame_of(t, beat0, bps, frames), self.rt_note, vel);
            }
            self.rt_next += 1;
        }
        self.rt_active = false; // all hits emitted
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
    use crate::sequencer::{Lock, LockParam, Pattern, Termination};

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
}
