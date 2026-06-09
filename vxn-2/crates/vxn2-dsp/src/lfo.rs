//! Low-frequency oscillators (ticket 0006 / ADR §4).
//!
//! Two flavours share waveform shapes and evaluation logic:
//!
//! - [`Lfo1`] — patch-global, free-running or BPM-synced. Scalar phase, one
//!   instance per patch, evaluated once per control block.
//! - [`Lfo2Stack`] — per-voice, lane-packed across [`STACK_LANES`]. One
//!   instance per [`crate::stack::Stack`], evaluated once per control block.
//!   Tracks per-stack delay + fade since note-on.
//!
//! Both are *control rate*: one phase-update + one output sample per block.
//! Block sizes up to a couple of ms — well below LFO bandwidths — so no
//! anti-aliasing is needed. At rates beyond `≈1 cycle / block` (e.g. 50 Hz at
//! a 1024-sample block @ 48 kHz), a single block may span more than one full
//! cycle; the phase still advances correctly via `u32` wrapping, but at most
//! one S&H value is drawn per block. Acceptable at control rate.
//!
//! ## Q32 phase convention
//!
//! Matches [`crate::sine`]: a full `u32` rotation = one LFO cycle. Free
//! wraparound via wrapping add. Q32 reading is shared with the operator core,
//! making `voice_rand → lfo2_phase` (mod matrix, ticket 0008) a Q32 add.
//!
//! ## Depth lives in the mod matrix
//!
//! Per the ticket: LFO depth is *not* part of the LFO struct. The faceplate
//! `lfo1_depth` / `lfo2_depth` knobs are macro multipliers applied at
//! matrix-source-eval time. The LFO produces raw bipolar `[-1, +1]` output.
//!
//! ## S&H — sample-and-hold on cycle boundary
//!
//! Detected by `wrapping_add` overflow: a new pseudo-random bipolar value is
//! latched at each wrap. Per-instance `u64` xorshift state so stacks
//! decorrelate naturally.
//!
//! ## BPM sync subdivisions
//!
//! Coarse→fine straight / dotted / triplet table identical to VXN1's
//! `vxn_app::sync::SUBDIVISIONS`. Reused here verbatim because the two
//! synths are sibling Cargo workspaces (per ADR §Consequences); duplication
//! is preferred over carving a shared crate before divergence stabilises.

use crate::sine::scalar::fast_sine_q32;
use crate::stack::STACK_LANES;

/// LFO waveform set. Six shapes shared by LFO1 + LFO2.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LfoShape {
    #[default]
    Sine,
    Triangle,
    SawUp,
    SawDown,
    Pulse,
    SampleHold,
}

impl LfoShape {
    pub const ALL: [LfoShape; 6] = [
        LfoShape::Sine,
        LfoShape::Triangle,
        LfoShape::SawUp,
        LfoShape::SawDown,
        LfoShape::Pulse,
        LfoShape::SampleHold,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LfoShape::Sine => "Sine",
            LfoShape::Triangle => "Tri",
            LfoShape::SawUp => "Saw+",
            LfoShape::SawDown => "Saw-",
            LfoShape::Pulse => "Pulse",
            LfoShape::SampleHold => "S&H",
        }
    }

    /// Q32 phase whose output is zero (rising). Used by KeySync retrigger so
    /// modulation eases out of zero rather than stepping to an extreme. Pulse
    /// and S&H have no zero crossing — restart at cycle boundary.
    #[inline]
    pub fn zero_crossing_q32(self) -> u32 {
        match self {
            LfoShape::Sine => 0,                  // sin(0) = 0, rising
            LfoShape::Triangle => 0x4000_0000,    // phase 0.25 → 0, rising
            LfoShape::SawUp => 0x8000_0000,       // 2p−1 = 0 at p=0.5
            LfoShape::SawDown => 0x8000_0000,     // 1−2p = 0 at p=0.5
            LfoShape::Pulse => 0,                 // no zero crossing
            LfoShape::SampleHold => 0,            // stepped
        }
    }
}

// --- shape evaluation -------------------------------------------------------

/// Bipolar `[-1, +1]` sample for `shape` at Q32 `phase`. `sh_value` is the
/// currently-held random sample (used only by `SampleHold`).
#[inline]
fn eval_shape(shape: LfoShape, phase: u32, sh_value: f32) -> f32 {
    match shape {
        LfoShape::Sine => fast_sine_q32(phase),
        LfoShape::Triangle => {
            let p = phase as f32 * (1.0 / 4_294_967_296.0);
            1.0 - 4.0 * (p - 0.5).abs()
        }
        LfoShape::SawUp => {
            let p = phase as f32 * (1.0 / 4_294_967_296.0);
            2.0 * p - 1.0
        }
        LfoShape::SawDown => {
            let p = phase as f32 * (1.0 / 4_294_967_296.0);
            1.0 - 2.0 * p
        }
        LfoShape::Pulse => {
            if phase < 0x8000_0000 {
                1.0
            } else {
                -1.0
            }
        }
        LfoShape::SampleHold => sh_value,
    }
}

/// Q32 phase increment per block for `hz` at `block_secs`. Computed in `f64`
/// to keep precision for slow rates; truncates to `u32` so multi-cycle blocks
/// drop the integer-cycle component (control-rate one-update-per-block).
#[inline]
fn phase_inc_q32(hz: f32, block_secs: f32) -> u32 {
    let inc = (hz as f64) * (block_secs as f64) * 4_294_967_296.0;
    inc as u64 as u32
}

#[inline]
fn xorshift_step(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// Bipolar `[-1, +1)` from the top 24 bits of a xorshift step.
#[inline]
fn xorshift_bipolar(state: &mut u64) -> f32 {
    let u = (xorshift_step(state) >> 40) as f32 * (1.0 / (1u64 << 24) as f32);
    u * 2.0 - 1.0
}

// --- BPM sync table ---------------------------------------------------------

/// One tempo-sync subdivision: label + length in beats per LFO cycle (quarter
/// note = 1 beat). Straight = base, dotted = ×1.5, triplet = ×2/3.
#[derive(Clone, Copy, Debug)]
pub struct Subdivision {
    pub label: &'static str,
    pub beats: f32,
}

const fn s(label: &'static str, beats: f32) -> Subdivision {
    Subdivision { label, beats }
}

const T: f32 = 2.0 / 3.0;

/// Coarse→fine table, straight / dotted / triplet, 1/1 … 1/32. Identical to
/// VXN1's `vxn_app::sync::SUBDIVISIONS` (intentional duplication — see module
/// docs).
pub static SUBDIVISIONS: [Subdivision; 18] = [
    s("1/1", 4.0),
    s("1/1.", 4.0 * 1.5),
    s("1/1T", 4.0 * T),
    s("1/2", 2.0),
    s("1/2.", 2.0 * 1.5),
    s("1/2T", 2.0 * T),
    s("1/4", 1.0),
    s("1/4.", 1.0 * 1.5),
    s("1/4T", 1.0 * T),
    s("1/8", 0.5),
    s("1/8.", 0.5 * 1.5),
    s("1/8T", 0.5 * T),
    s("1/16", 0.25),
    s("1/16.", 0.25 * 1.5),
    s("1/16T", 0.25 * T),
    s("1/32", 0.125),
    s("1/32.", 0.125 * 1.5),
    s("1/32T", 0.125 * T),
];

/// Map a normalised `[0, 1]` rate-fader position to a subdivision index.
#[inline]
pub fn index_from_norm(norm: f32) -> usize {
    let last = SUBDIVISIONS.len() - 1;
    (norm.clamp(0.0, 1.0) * last as f32).round() as usize
}

/// Hz at `tempo_bpm` for the subdivision at `index`.
#[inline]
pub fn synced_hz(tempo_bpm: f32, index: usize) -> f32 {
    let beats = SUBDIVISIONS[index.min(SUBDIVISIONS.len() - 1)].beats;
    (tempo_bpm / 60.0) / beats
}

// --- LFO1 (global) ----------------------------------------------------------

/// LFO1 patch params. Depth (a macro multiplier) lives at the matrix-source
/// boundary, not here — see module docs.
#[derive(Clone, Copy, Debug)]
pub struct Lfo1Params {
    pub shape: LfoShape,
    /// Free-running Hz. Used when `sync = false`.
    pub rate_hz: f32,
    pub sync: bool,
    /// Subdivision index when `sync = true`. See [`SUBDIVISIONS`].
    pub sync_index: usize,
}

impl Default for Lfo1Params {
    fn default() -> Self {
        Self {
            shape: LfoShape::Sine,
            rate_hz: 2.4,
            sync: false,
            sync_index: 6, // 1/4
        }
    }
}

/// Patch-global LFO. One instance per patch.
#[derive(Clone, Copy, Debug)]
pub struct Lfo1 {
    pub phase: u32,
    pub sh_state: u64,
    pub sh_value: f32,
}

impl Default for Lfo1 {
    fn default() -> Self {
        Self {
            phase: 0,
            sh_state: 0xA5A5_5A5A_DEAD_BEEF,
            sh_value: 0.0,
        }
    }
}

impl Lfo1 {
    pub fn new(seed: u64) -> Self {
        Self {
            phase: 0,
            sh_state: if seed == 0 { 0xDEAD_BEEF_DEAD_BEEF } else { seed },
            sh_value: 0.0,
        }
    }

    /// Reset on host transport restart. Sync mode anchors LFO1 to the bar
    /// grid — restart events realign it.
    #[inline]
    pub fn reset_phase(&mut self) {
        self.phase = 0;
    }

    /// Advance one control block and return the bipolar `[-1, +1]` output.
    #[inline]
    pub fn eval(&mut self, params: &Lfo1Params, tempo_bpm: f32, block_secs: f32) -> f32 {
        let hz = if params.sync {
            synced_hz(tempo_bpm, params.sync_index)
        } else {
            params.rate_hz
        };
        let inc = phase_inc_q32(hz, block_secs);
        let (new_phase, wrapped) = self.phase.overflowing_add(inc);
        self.phase = new_phase;
        if wrapped {
            self.sh_value = xorshift_bipolar(&mut self.sh_state);
        }
        eval_shape(params.shape, self.phase, self.sh_value)
    }
}

// --- LFO2 (per-voice, lane-packed) ------------------------------------------
//
// LFO2 is always key-triggered: on every note-on the lane phases retrigger to
// the shape's zero crossing and delay+fade restart from zero. The free-running
// variant tracked by an earlier `Lfo2Trig` enum was removed when the UI
// dropped the Trig switch in favour of a host-tempo Sync toggle (matching
// VXN1's per-voice LFO behaviour).

#[derive(Clone, Copy, Debug)]
pub struct Lfo2Params {
    pub shape: LfoShape,
    /// Free-running Hz. Used when `sync = false`.
    pub rate_hz: f32,
    pub delay_ms: f32,
    pub fade_ms: f32,
    /// When true, `rate_hz` is ignored and the rate is taken from
    /// `sync_index` against host tempo (see [`SUBDIVISIONS`]).
    pub sync: bool,
    /// Subdivision index when `sync = true`.
    pub sync_index: usize,
}

impl Default for Lfo2Params {
    fn default() -> Self {
        Self {
            shape: LfoShape::SawUp,
            rate_hz: 5.1,
            delay_ms: 180.0,
            fade_ms: 320.0,
            sync: false,
            sync_index: 6, // 1/4
        }
    }
}

/// LFO2 lane-packed across the [`STACK_LANES`] of one [`crate::stack::Stack`].
/// All eight lanes share `Lfo2Params` + delay/fade state, but each has its
/// own phase and S&H. Matrix `voice_rand → lfo2_phase` (ticket 0008) writes
/// per-lane phase offsets to scatter them — that's where the supersaw
/// shimmer comes from.
#[derive(Clone, Copy, Debug)]
pub struct Lfo2Stack {
    pub phase: [u32; STACK_LANES],
    pub sh_state: [u64; STACK_LANES],
    pub sh_value: [f32; STACK_LANES],
    /// Seconds since the most recent note-on (KeySync) — drives delay+fade.
    /// In Free mode this stays at a large value so the envelope is full.
    pub secs_since_on: f32,
}

impl Default for Lfo2Stack {
    fn default() -> Self {
        // Per-lane fixed seeds; in real use [`Lfo2Stack::reseed`] is called
        // from `Stack::note_on` with a stack-derived seed.
        let mut sh_state = [0u64; STACK_LANES];
        for (k, slot) in sh_state.iter_mut().enumerate() {
            *slot = 0x9E37_79B9_7F4A_7C15u64.wrapping_mul((k as u64).wrapping_add(1));
        }
        Self {
            phase: [0; STACK_LANES],
            sh_state,
            sh_value: [0.0; STACK_LANES],
            // Free-mode default: envelope already past delay+fade.
            secs_since_on: f32::INFINITY,
        }
    }
}

impl Lfo2Stack {
    /// Reseed per-lane S&H states from a single u64 seed (e.g. the stack's
    /// note-on seed). Lanes spread out via xorshift to keep them
    /// statistically independent.
    pub fn reseed(&mut self, mut seed: u64) {
        if seed == 0 {
            seed = 0xDEAD_BEEF_DEAD_BEEF;
        }
        for slot in &mut self.sh_state {
            xorshift_step(&mut seed);
            *slot = if seed == 0 { 0xDEAD_BEEF_DEAD_BEEF } else { seed };
        }
    }

    /// Note-on handling. All lanes' phase → shape's zero crossing, delay+fade
    /// timer reset. LFO2 is always key-triggered.
    pub fn note_on(&mut self, params: &Lfo2Params) {
        let q = params.shape.zero_crossing_q32();
        for k in 0..STACK_LANES {
            self.phase[k] = q;
        }
        self.secs_since_on = 0.0;
    }

    /// Advance one control block; return per-lane bipolar outputs post-delay,
    /// post-fade. Lanes whose delay hasn't elapsed read 0; lanes in the fade
    /// window are linearly ramped from 0 → full.
    #[inline]
    pub fn eval(
        &mut self,
        params: &Lfo2Params,
        tempo_bpm: f32,
        block_secs: f32,
    ) -> [f32; STACK_LANES] {
        let hz = if params.sync {
            synced_hz(tempo_bpm, params.sync_index)
        } else {
            params.rate_hz
        };
        let inc = phase_inc_q32(hz, block_secs);
        // Advance secs_since_on, saturating to keep f32 finite.
        if self.secs_since_on.is_finite() {
            self.secs_since_on += block_secs;
        }
        let env = self.envelope(params);
        let mut out = [0.0_f32; STACK_LANES];
        for k in 0..STACK_LANES {
            let (new_phase, wrapped) = self.phase[k].overflowing_add(inc);
            self.phase[k] = new_phase;
            if wrapped && params.shape == LfoShape::SampleHold {
                self.sh_value[k] = xorshift_bipolar(&mut self.sh_state[k]);
            }
            out[k] = eval_shape(params.shape, self.phase[k], self.sh_value[k]) * env;
        }
        out
    }

    /// Current delay+fade envelope in `[0, 1]`.
    #[inline]
    fn envelope(&self, params: &Lfo2Params) -> f32 {
        let t_ms = self.secs_since_on * 1000.0;
        if t_ms < params.delay_ms {
            0.0
        } else if t_ms < params.delay_ms + params.fade_ms {
            if params.fade_ms <= 0.0 {
                1.0
            } else {
                (t_ms - params.delay_ms) / params.fade_ms
            }
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLK: f32 = 64.0 / 48_000.0;

    fn run_lfo1(params: &Lfo1Params, blocks: usize) -> Vec<f32> {
        let mut lfo = Lfo1::default();
        let mut out = Vec::with_capacity(blocks);
        for _ in 0..blocks {
            out.push(lfo.eval(params, 120.0, BLK));
        }
        out
    }

    #[test]
    fn shape_labels_match() {
        assert_eq!(LfoShape::SampleHold.label(), "S&H");
        assert_eq!(LfoShape::SawDown.label(), "Saw-");
    }

    #[test]
    fn lfo1_period_matches_rate() {
        // 1 Hz at 64-sample blocks @ 48 kHz → 750 blocks per cycle.
        let params = Lfo1Params {
            shape: LfoShape::Sine,
            rate_hz: 1.0,
            sync: false,
            sync_index: 0,
        };
        let samples = run_lfo1(&params, 2000);
        let mut crossings = vec![];
        let mut prev = samples[0];
        for (i, &v) in samples.iter().enumerate().skip(1) {
            if prev < 0.0 && v >= 0.0 {
                crossings.push(i);
            }
            prev = v;
        }
        assert!(crossings.len() >= 2);
        let period = (crossings[1] - crossings[0]) as i32;
        assert!((period - 750).abs() <= 4, "period {period}, want ≈750");
    }

    #[test]
    fn lfo1_all_shapes_bipolar_bounded() {
        for shape in LfoShape::ALL {
            let params = Lfo1Params {
                shape,
                rate_hz: 5.0,
                sync: false,
                sync_index: 0,
            };
            let samples = run_lfo1(&params, 4000);
            for v in samples {
                assert!(v.is_finite() && v.abs() <= 1.001, "{shape:?} {v}");
            }
        }
    }

    #[test]
    fn lfo1_sample_hold_steps_at_cycle_boundary() {
        let params = Lfo1Params {
            shape: LfoShape::SampleHold,
            rate_hz: 4.0,
            sync: false,
            sync_index: 0,
        };
        // ~187 blocks per cycle @ 64-sample blocks, 48 kHz, 4 Hz → ~5 cycles
        // across 1000 blocks → ~5 distinct held values.
        let samples = run_lfo1(&params, 1000);
        let mut steps = 0;
        let mut prev = samples[0];
        for &v in &samples[1..] {
            if v != prev {
                steps += 1;
                prev = v;
            }
        }
        assert!(
            (3..8).contains(&steps),
            "S&H steps={steps}, want ~5 over 1000 blocks"
        );
    }

    #[test]
    fn lfo1_sync_overrides_free_rate() {
        // 1/4 @ 120 BPM = 2 Hz, regardless of free rate_hz.
        let q_idx = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        let params = Lfo1Params {
            shape: LfoShape::Sine,
            rate_hz: 99.0, // ignored when sync on
            sync: true,
            sync_index: q_idx,
        };
        let samples = run_lfo1(&params, 1500);
        let mut crossings = vec![];
        let mut prev = samples[0];
        for (i, &v) in samples.iter().enumerate().skip(1) {
            if prev < 0.0 && v >= 0.0 {
                crossings.push(i);
            }
            prev = v;
        }
        // 2 Hz at 64-sample blocks @ 48 kHz = 375 blocks per cycle.
        let period = (crossings[1] - crossings[0]) as i32;
        assert!((period - 375).abs() <= 3, "synced period {period}, want ≈375");
    }

    #[test]
    fn lfo1_reset_phase_realigns_sync() {
        let mut lfo = Lfo1::default();
        let p = Lfo1Params {
            shape: LfoShape::Sine,
            rate_hz: 4.0,
            sync: false,
            sync_index: 0,
        };
        for _ in 0..100 {
            lfo.eval(&p, 120.0, BLK);
        }
        lfo.reset_phase();
        assert_eq!(lfo.phase, 0);
    }

    #[test]
    fn synced_hz_matches_beat_math() {
        let q = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        assert!((synced_hz(120.0, q) - 2.0).abs() < 1e-5);
        let e = SUBDIVISIONS.iter().position(|s| s.label == "1/8").unwrap();
        assert!((synced_hz(120.0, e) - 4.0).abs() < 1e-5);
    }

    // --- LFO2 -------------------------------------------------------------

    fn run_lfo2(
        lfo: &mut Lfo2Stack,
        params: &Lfo2Params,
        blocks: usize,
    ) -> Vec<[f32; STACK_LANES]> {
        let mut out = Vec::with_capacity(blocks);
        for _ in 0..blocks {
            out.push(lfo.eval(params, 120.0, BLK));
        }
        out
    }

    #[test]
    fn lfo2_delay_outputs_zero_then_fade_then_full() {
        let mut lfo = Lfo2Stack::default();
        let params = Lfo2Params {
            shape: LfoShape::SawUp,
            rate_hz: 10.0,
            delay_ms: 50.0,
            fade_ms: 50.0,
            sync: false,
            sync_index: 0,
        };
        lfo.note_on(&params);
        // BLK ≈ 1.333 ms; 38 blocks ≈ 50.6 ms (just past delay end).
        let pre = run_lfo2(&mut lfo, &params, 20); // ~26.7 ms — fully inside delay
        for sample in pre {
            for v in sample {
                assert_eq!(v, 0.0, "expected silent during delay");
            }
        }
        // Run until past delay + fade ≈ 100 ms total.
        let post = run_lfo2(&mut lfo, &params, 100);
        // Should see non-zero values by the end.
        let last = post.last().unwrap();
        let any_nonzero = last.iter().any(|v| v.abs() > 0.05);
        assert!(any_nonzero, "expected non-zero past delay+fade, got {last:?}");
    }

    #[test]
    fn lfo2_zero_delay_outputs_signal_immediately() {
        let mut lfo = Lfo2Stack::default();
        let params = Lfo2Params {
            shape: LfoShape::SawUp,
            rate_hz: 10.0,
            delay_ms: 0.0,
            fade_ms: 0.0,
            sync: false,
            sync_index: 0,
        };
        lfo.note_on(&params);
        let out = lfo.eval(&params, 120.0, BLK);
        // SawUp at phase 0 starts at −1; with zero delay+fade the envelope is
        // already full so a non-zero sample must reach the output on tick 1.
        let any_nonzero = out.iter().any(|v| v.abs() > 0.001);
        assert!(any_nonzero, "expected non-zero with zero delay, got {out:?}");
    }

    #[test]
    fn lfo2_keysync_resets_phase_to_zero_crossing() {
        let mut lfo = Lfo2Stack::default();
        let params = Lfo2Params {
            shape: LfoShape::Triangle,
            rate_hz: 4.0,
            delay_ms: 0.0,
            fade_ms: 0.0,
            sync: false,
            sync_index: 0,
        };
        // Run a while to scramble phase.
        lfo.note_on(&params);
        for _ in 0..200 {
            lfo.eval(&params, 120.0, BLK);
        }
        lfo.note_on(&params);
        let expected = LfoShape::Triangle.zero_crossing_q32();
        for k in 0..STACK_LANES {
            assert_eq!(lfo.phase[k], expected, "lane {k} not retriggered");
        }
    }

    #[test]
    fn lfo2_lanes_with_distinct_phases_produce_distinct_outputs() {
        // Simulates `voice_rand → lfo2_phase` (matrix slot): poke per-lane
        // starting phases after note_on and observe outputs decorrelate.
        let mut lfo = Lfo2Stack::default();
        let params = Lfo2Params {
            shape: LfoShape::Sine,
            rate_hz: 4.0,
            delay_ms: 0.0,
            fade_ms: 0.0,
            sync: false,
            sync_index: 0,
        };
        lfo.note_on(&params);
        for k in 0..STACK_LANES {
            lfo.phase[k] = (k as u32) * (u32::MAX / STACK_LANES as u32);
        }
        let out = lfo.eval(&params, 120.0, BLK);
        let mut distinct = std::collections::HashSet::new();
        for v in out {
            distinct.insert(v.to_bits());
        }
        assert!(
            distinct.len() >= STACK_LANES - 1,
            "phases scattered should produce distinct lane outputs: {out:?}"
        );
    }

    #[test]
    fn lfo2_all_shapes_bipolar_bounded() {
        let mut lfo = Lfo2Stack::default();
        for shape in LfoShape::ALL {
            let params = Lfo2Params {
                shape,
                rate_hz: 5.0,
                delay_ms: 0.0,
                fade_ms: 0.0,
                sync: false,
                sync_index: 0,
            };
            lfo.note_on(&params);
            for _ in 0..2000 {
                let out = lfo.eval(&params, 120.0, BLK);
                for v in out {
                    assert!(v.is_finite() && v.abs() <= 1.001, "{shape:?} {v}");
                }
            }
        }
    }

    #[test]
    fn lfo2_sample_hold_steps_at_cycle_boundary() {
        let mut lfo = Lfo2Stack::default();
        let params = Lfo2Params {
            shape: LfoShape::SampleHold,
            rate_hz: 4.0,
            delay_ms: 0.0,
            fade_ms: 0.0,
            sync: false,
            sync_index: 0,
        };
        lfo.note_on(&params);
        // First eval initialises sh_value from the wrap on advance.
        let _ = lfo.eval(&params, 120.0, BLK);
        let initial = lfo.sh_value[0];
        // Run many blocks; observe sh_value changes for lane 0.
        let mut distinct = std::collections::HashSet::new();
        distinct.insert(initial.to_bits());
        for _ in 0..1000 {
            lfo.eval(&params, 120.0, BLK);
            distinct.insert(lfo.sh_value[0].to_bits());
        }
        assert!(distinct.len() > 5, "S&H lane 0 produced too few values: {}", distinct.len());
    }

    #[test]
    fn lfo2_sync_overrides_free_rate() {
        // 1/4 @ 120 BPM = 2 Hz, regardless of free rate_hz.
        let q_idx = SUBDIVISIONS.iter().position(|s| s.label == "1/4").unwrap();
        let mut lfo = Lfo2Stack::default();
        let params = Lfo2Params {
            shape: LfoShape::Sine,
            rate_hz: 99.0, // ignored when sync on
            delay_ms: 0.0,
            fade_ms: 0.0,
            sync: true,
            sync_index: q_idx,
        };
        lfo.note_on(&params);
        let mut crossings = vec![];
        let mut prev = lfo.eval(&params, 120.0, BLK)[0];
        for i in 1..1500 {
            let v = lfo.eval(&params, 120.0, BLK)[0];
            if prev < 0.0 && v >= 0.0 {
                crossings.push(i);
            }
            prev = v;
        }
        // 2 Hz at 64-sample blocks @ 48 kHz = 375 blocks per cycle.
        let period = (crossings[1] - crossings[0]) as i32;
        assert!((period - 375).abs() <= 3, "synced period {period}, want ≈375");
    }

    #[test]
    fn lfo2_reseed_separates_lane_state() {
        let mut lfo = Lfo2Stack::default();
        lfo.reseed(0xABCD_1234_5678_9ABC);
        let states = lfo.sh_state;
        let mut distinct = std::collections::HashSet::new();
        for s in states {
            distinct.insert(s);
        }
        assert_eq!(distinct.len(), STACK_LANES, "lane states collided");
    }
}
