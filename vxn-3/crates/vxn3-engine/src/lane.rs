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

use crate::sequencer::{Pattern, RetrigCurve};

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
        }
    }

    /// Reset transport-derived phase + in-flight retrig (transport stop / engine
    /// reset). The PRNG stream is left running.
    pub fn reset(&mut self) {
        self.expected_beat = f64::NEG_INFINITY;
        self.last_index = i64::MIN;
        self.rt_active = false;
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
