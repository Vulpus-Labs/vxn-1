//! Voice stack: up to 8 concurrent operator-voice instances sharing one note,
//! processed in SoA lockstep for SIMD-friendly inner loops.
//!
//! Ticket 0005 deliverable. ADR §3: a played note instantiates `density`
//! parallel op-voice instances, each carrying `voice_idx`, `voice_spread`,
//! `voice_rand` for mod-matrix routing. Macro knobs (detune / spread / phase /
//! distrib) precompute per-lane offsets at note-on; the per-sample hot path is
//! branch-free.
//!
//! ## Lane packing
//!
//! [`STACK_LANES`] = 8 is the fixed packed width regardless of `density`.
//! Lanes `0..density` carry active instances; lanes `density..8` stay silent
//! via a precomputed pan mask that zeroes inactive carrier-fold gains. No
//! per-lane branches inside the sample loop — autovectorisation is the goal
//! (verified via asm dump per ticket acceptance criterion).
//!
//! ## SoA layout
//!
//! Per stack op: `phase[8]`, `phase_inc[8]`, `fb_prev1[8]`, `fb_prev2[8]`,
//! `fb_scale[8]` as contiguous arrays LLVM lowers to 2× NEON 4-wide registers
//! on AArch64. Per-op scalars (EG) live alongside, shared
//! across the 8 lanes.
//!
//! ## Algorithm routing
//!
//! Mirrors [`crate::algo`]'s one-fn-per-algo dispatch but lane-packed.
//! [`LaneRouteFn`] takes a 6 × 8 prev-output matrix and returns the matching
//! mod-input matrix + per-lane carrier sum. Routing is per-edge accumulation
//! across all 8 lanes — straight-line code, vectorises trivially.
//!
//! ## RNG
//!
//! Single u64 xorshift state, seeded per note-on from `(note, velocity,
//! counter)`. Reproducible across re-renders — essential for deterministic
//! offline rendering (ticket Notes).

use crate::algo::{N_ALGOS, N_OPS, spec_of};
use crate::eg::EgStage;
use crate::envelope::{ModEnvState, PitchEgState};
use crate::ks::{ks_level_mult, ks_rate_mult};
use crate::lfo::Lfo2Stack;
use crate::op::{OpParams, PM_SCALE_Q32, RatioMode, midi_to_hz};
use crate::sine::scalar::fast_sine_q32;
use crate::tables::{fb_scale, vel_factor};
use crate::voice::VoiceParams;

/// Fixed packed-lane width. All stack DSP runs over 8 lanes; `density < 8`
/// silences the trailing lanes via the pan mask.
pub const STACK_LANES: usize = 8;

/// Distribution mode for `voice_spread` across lanes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StackDistrib {
    /// Even spacing in `[-1, +1]`.
    #[default]
    Linear,
    /// Exponentially clusters toward the outer lanes.
    Geometric,
    /// Each lane draws a fresh `voice_spread` from the RNG per note-on.
    Random,
}

/// Stack-macro parameters. Convenience knobs that write into per-lane
/// offsets at note-on. ADR §3: equivalent to pre-wired matrix slots; the
/// matrix can layer additional routings on top.
#[derive(Clone, Copy, Debug)]
pub struct StackParams {
    /// Number of active lanes (1..=8).
    pub density: u8,
    /// Maximum detune across the stack in cents. Outer lanes detune by
    /// ±`detune_cents_max`; centre lane is at zero.
    pub detune_cents_max: f32,
    /// Stereo pan spread across lanes in `[0, 1]`. Multiplied against
    /// `voice_spread` to produce the per-lane pan offset.
    pub spread: f32,
    /// Phase decorrelation across lanes in `[0, 1]`. Multiplied against
    /// `voice_rand` to produce the per-lane Q32 phase offset applied to
    /// all six ops at note-on.
    pub phase: f32,
    pub distrib: StackDistrib,
}

impl Default for StackParams {
    fn default() -> Self {
        Self {
            density: 4,
            detune_cents_max: 8.0,
            spread: 0.60,
            phase: 0.50,
            distrib: StackDistrib::Linear,
        }
    }
}

/// Per-algorithm lane-batched router. Takes the prior-sample 6 × 8 output
/// matrix and emits the next-sample mod-input matrix + per-lane carrier sum.
pub type LaneRouteFn = fn(
    prev: &[[f32; STACK_LANES]; N_OPS],
) -> ([[f32; STACK_LANES]; N_OPS], [f32; STACK_LANES]);

macro_rules! impl_lane_route {
    (
        $name:ident,
        edges = [$(($m:literal, $c:literal)),* $(,)?],
        carriers = [$($cs:literal),* $(,)?]
    ) => {
        #[inline(never)]
        #[allow(unused_mut)]
        fn $name(
            prev: &[[f32; STACK_LANES]; N_OPS],
        ) -> ([[f32; STACK_LANES]; N_OPS], [f32; STACK_LANES]) {
            let mut mi = [[0.0_f32; STACK_LANES]; N_OPS];
            $(
                for k in 0..STACK_LANES {
                    mi[$c - 1][k] += prev[$m - 1][k];
                }
            )*
            let mut cs = [0.0_f32; STACK_LANES];
            for k in 0..STACK_LANES {
                cs[k] = 0.0_f32 $( + prev[$cs - 1][k] )*;
            }
            (mi, cs)
        }
    };
}

impl_lane_route!(lane_route_algo_1,  edges = [(2,1),(4,3),(5,4),(6,5)],          carriers = [1,3]);
impl_lane_route!(lane_route_algo_2,  edges = [(2,1),(4,3),(5,4),(6,5)],          carriers = [1,3]);
impl_lane_route!(lane_route_algo_3,  edges = [(2,1),(3,2),(5,4),(6,5)],          carriers = [1,4]);
impl_lane_route!(lane_route_algo_4,  edges = [(2,1),(3,2),(5,4),(6,5)],          carriers = [1,4]);
impl_lane_route!(lane_route_algo_5,  edges = [(2,1),(4,3),(6,5)],                carriers = [1,3,5]);
impl_lane_route!(lane_route_algo_6,  edges = [(2,1),(4,3),(6,5)],                carriers = [1,3,5]);
impl_lane_route!(lane_route_algo_7,  edges = [(2,1),(4,3),(5,3),(6,5)],          carriers = [1,3]);
impl_lane_route!(lane_route_algo_8,  edges = [(2,1),(4,3),(5,3),(6,5)],          carriers = [1,3]);
impl_lane_route!(lane_route_algo_9,  edges = [(2,1),(4,3),(5,3),(6,5)],          carriers = [1,3]);
impl_lane_route!(lane_route_algo_10, edges = [(2,1),(3,2),(5,4),(6,4)],          carriers = [1,4]);
impl_lane_route!(lane_route_algo_11, edges = [(2,1),(3,2),(5,4),(6,4)],          carriers = [1,4]);
impl_lane_route!(lane_route_algo_12, edges = [(2,1),(4,3),(5,3),(6,3)],          carriers = [1,3]);
impl_lane_route!(lane_route_algo_13, edges = [(2,1),(4,3),(5,3),(6,3)],          carriers = [1,3]);
impl_lane_route!(lane_route_algo_14, edges = [(2,1),(4,3),(5,4),(6,4)],          carriers = [1,3]);
impl_lane_route!(lane_route_algo_15, edges = [(2,1),(4,3),(5,4),(6,4)],          carriers = [1,3]);
impl_lane_route!(lane_route_algo_16, edges = [(2,1),(3,1),(4,3),(5,3),(6,5)],    carriers = [1]);
impl_lane_route!(lane_route_algo_17, edges = [(2,1),(3,1),(4,3),(5,3),(6,5)],    carriers = [1]);
impl_lane_route!(lane_route_algo_18, edges = [(2,1),(3,1),(4,3),(5,4),(6,4)],    carriers = [1]);
impl_lane_route!(lane_route_algo_19, edges = [(2,1),(3,2),(6,4),(6,5)],          carriers = [1,4,5]);
impl_lane_route!(lane_route_algo_20, edges = [(3,1),(3,2),(5,4),(6,4)],          carriers = [1,2,4]);
impl_lane_route!(lane_route_algo_21, edges = [(3,1),(3,2),(6,4),(6,5)],          carriers = [1,2,4,5]);
impl_lane_route!(lane_route_algo_22, edges = [(2,1),(6,3),(6,4),(6,5)],          carriers = [1,3,4,5]);
impl_lane_route!(lane_route_algo_23, edges = [(3,2),(6,4),(6,5)],                carriers = [1,2,4,5]);
impl_lane_route!(lane_route_algo_24, edges = [(6,3),(6,4),(6,5)],                carriers = [1,2,3,4,5]);
impl_lane_route!(lane_route_algo_25, edges = [(6,4),(6,5)],                      carriers = [1,2,3,4,5]);
impl_lane_route!(lane_route_algo_26, edges = [(3,2),(5,4),(6,4)],                carriers = [1,2,4]);
impl_lane_route!(lane_route_algo_27, edges = [(3,2),(5,4),(6,4)],                carriers = [1,2,4]);
impl_lane_route!(lane_route_algo_28, edges = [(2,1),(4,3),(5,4)],                carriers = [1,3,6]);
impl_lane_route!(lane_route_algo_29, edges = [(4,3),(6,5)],                      carriers = [1,2,3,5]);
impl_lane_route!(lane_route_algo_30, edges = [(4,3),(5,4)],                      carriers = [1,2,3,6]);
impl_lane_route!(lane_route_algo_31, edges = [(6,5)],                            carriers = [1,2,3,4,5]);
impl_lane_route!(lane_route_algo_32, edges = [],                                 carriers = [1,2,3,4,5,6]);

pub static LANE_ROUTE_FNS: [LaneRouteFn; N_ALGOS] = [
    lane_route_algo_1,  lane_route_algo_2,  lane_route_algo_3,  lane_route_algo_4,
    lane_route_algo_5,  lane_route_algo_6,  lane_route_algo_7,  lane_route_algo_8,
    lane_route_algo_9,  lane_route_algo_10, lane_route_algo_11, lane_route_algo_12,
    lane_route_algo_13, lane_route_algo_14, lane_route_algo_15, lane_route_algo_16,
    lane_route_algo_17, lane_route_algo_18, lane_route_algo_19, lane_route_algo_20,
    lane_route_algo_21, lane_route_algo_22, lane_route_algo_23, lane_route_algo_24,
    lane_route_algo_25, lane_route_algo_26, lane_route_algo_27, lane_route_algo_28,
    lane_route_algo_29, lane_route_algo_30, lane_route_algo_31, lane_route_algo_32,
];

/// Resolve `algo` (1..=32) to its lane-packed router. Use once per block /
/// note-on, then call per sample.
#[inline]
pub fn resolve_lane_route(algo: u8) -> LaneRouteFn {
    let idx = (algo.clamp(1, N_ALGOS as u8) - 1) as usize;
    LANE_ROUTE_FNS[idx]
}

/// Per-op runtime state, lane-packed.
#[derive(Clone, Copy, Debug, Default)]
pub struct StackOp {
    pub phase: [u32; STACK_LANES],
    pub phase_inc: [u32; STACK_LANES],
    pub fb_prev1: [f32; STACK_LANES],
    pub fb_prev2: [f32; STACK_LANES],
    /// Pre-bend/glide cooked phase increment per lane. Source-of-truth for
    /// `apply_pitch_mult()` — `phase_inc[k] = base_phase_inc[k] * mult`.
    pub base_phase_inc: [u32; STACK_LANES],
    /// Per-op envelope; shared across lanes (one EG, one level per op).
    pub eg: crate::eg::EgState,
    /// Cooked feedback gain per lane. Non-zero only on the algorithm's
    /// structural FB op; per-lane because the matrix `Feedback` dest is a
    /// voice property (each lane can carry its own modulated amount).
    pub fb_scale: [f32; STACK_LANES],
}

/// One voice stack — six lane-packed ops + per-stack metadata.
#[derive(Clone, Copy, Debug)]
pub struct Stack {
    pub ops: [StackOp; N_OPS],
    /// Prev-sample op outputs per lane, fed to the router each tick.
    pub prev_outs: [[f32; STACK_LANES]; N_OPS],
    pub note: u8,
    pub velocity: u8,
    pub gate: bool,
    pub density: u8,
    /// Lane-instance index (0..density−1 in active lanes, 0 in trailing).
    pub voice_idx: [u8; STACK_LANES],
    /// Symmetric stack position per lane in `[-1, +1]`.
    pub voice_spread: [f32; STACK_LANES],
    /// Per-lane random in `[0, 1)`. Drawn from the stack RNG at note-on.
    pub voice_rand: [f32; STACK_LANES],
    /// Per-carrier-op, per-lane stereo gain pre-multiplied with the
    /// equal-power pan curve, the carrier mask, and the active-lane mask.
    /// Inactive lanes and non-carrier ops are zeroed — no per-sample branch.
    pub pan_l: [[f32; STACK_LANES]; N_OPS],
    pub pan_r: [[f32; STACK_LANES]; N_OPS],
    pub bend_st: f32,
    pub glide_st: f32,
    pub algo: u8,
    pub route_fn: LaneRouteFn,
    /// Per-voice LFO2, lane-packed across the 8 stack lanes (ticket 0006).
    pub lfo2: Lfo2Stack,
    /// Patch-wide Pitch EG (ticket 0007). Shared across stack lanes — same
    /// precedent as the per-op EG. Output is in semitones; default routing
    /// adds into the voice pitch sum.
    pub pitch_eg: PitchEgState,
    /// Patch-wide Mod Env (ticket 0007). Matrix-only source; ticks alongside
    /// the per-op EGs so 0008 can read its level without per-block coupling.
    pub mod_env: ModEnvState,
    /// Per-op × per-lane additive level offset, written by the mod matrix at
    /// block start and read by [`stack_tick_stereo`] / [`stack_tick_mono`].
    /// Effective level per sample is `(op.eg.level + op_level_mod[i][k])
    /// .clamp(0.0, 1.0)`. Zero when no matrix slot targets `OpNLevel`.
    pub op_level_mod: [[f32; STACK_LANES]; N_OPS],
    /// Per-lane pitch offset in semitones from matrix `GlobalPitch`. Summed
    /// with `bend_st + glide_st + pitch_eg.level_st` in
    /// [`Self::apply_pitch_mult`].
    pub global_pitch_mod_st: [f32; STACK_LANES],
    /// Per-op × per-lane pitch offset in semitones (sum of matrix `OpNRatio`
    /// + `OpNDetune` contributions). Both routes are semitone-shaped for
    /// matrix purposes — see DestId docs.
    pub op_pitch_mod_st: [[f32; STACK_LANES]; N_OPS],
    /// Per-op × per-lane pan offset, added to the cached base pan before the
    /// equal-power curve. Applied by [`Self::refresh_pan_with_mod`].
    pub op_pan_mod: [[f32; STACK_LANES]; N_OPS],
    /// Cached stack-macro spread, captured at note-on so the per-block pan
    /// refresh doesn't need a fresh `StackParams` handle.
    pub cached_spread: f32,
    /// Cached per-op static pan values from voice params at note-on. Source of
    /// truth for [`Self::refresh_pan_with_mod`]; never overwritten between
    /// note-ons.
    pub cached_op_pans: [f32; N_OPS],
}

impl Default for Stack {
    fn default() -> Self {
        Self {
            ops: [StackOp::default(); N_OPS],
            prev_outs: [[0.0_f32; STACK_LANES]; N_OPS],
            note: 0,
            velocity: 0,
            gate: false,
            density: 1,
            voice_idx: [0; STACK_LANES],
            voice_spread: [0.0; STACK_LANES],
            voice_rand: [0.0; STACK_LANES],
            pan_l: [[0.0_f32; STACK_LANES]; N_OPS],
            pan_r: [[0.0_f32; STACK_LANES]; N_OPS],
            bend_st: 0.0,
            glide_st: 0.0,
            algo: 1,
            route_fn: resolve_lane_route(1),
            lfo2: Lfo2Stack::default(),
            pitch_eg: PitchEgState::default(),
            mod_env: ModEnvState::default(),
            op_level_mod: [[0.0_f32; STACK_LANES]; N_OPS],
            global_pitch_mod_st: [0.0_f32; STACK_LANES],
            op_pitch_mod_st: [[0.0_f32; STACK_LANES]; N_OPS],
            op_pan_mod: [[0.0_f32; STACK_LANES]; N_OPS],
            cached_spread: 0.0,
            cached_op_pans: [0.0_f32; N_OPS],
        }
    }
}

/// xorshift64* — single u64 state, produces `[0, 1)` f32s.
#[inline]
fn xorshift_step(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

#[inline]
fn xorshift_f32(state: &mut u64) -> f32 {
    // Top 24 bits → [0, 1) — avoids the well-known low-bit weakness.
    (xorshift_step(state) >> 40) as f32 * (1.0 / (1u64 << 24) as f32)
}

/// Stable seed for an allocation: derived from note + velocity + counter so
/// the same note-on event produces identical lane offsets across re-renders.
#[inline]
pub fn stack_seed(note: u8, velocity: u8, counter: u64) -> u64 {
    let n = (note as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let v = (velocity as u64).wrapping_mul(0xBB67_AE85_84CA_A73B);
    let c = counter.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    let seed = n ^ v ^ c;
    if seed == 0 { 0xDEAD_BEEF_DEAD_BEEF } else { seed }
}

impl Stack {
    /// Live algorithm swap. Called every block from `Engine::apply_block_params`
    /// so a user-driven algo change in the picker re-routes the held note's
    /// op summing on the next block — without waiting for the next note-on
    /// to re-cook routing. No-op when the algo hasn't moved.
    #[inline]
    pub fn set_algo_live(&mut self, algo: u8) {
        if self.algo == algo {
            return;
        }
        self.algo = algo;
        self.route_fn = resolve_lane_route(algo);
    }

    /// Apply a uniform feedback amount, continuous `[0.0, 7.0]`, to every
    /// lane. Writes `fb_scale` onto the algorithm's structural FB op only;
    /// every other op's `fb_scale` is zeroed. Called from `note_on` and from
    /// `Engine::apply_block_params` each block so picker / fader changes
    /// propagate to a held note.
    #[inline]
    pub fn set_feedback_live(&mut self, feedback: f32) {
        self.write_fb_scales([fb_scale(feedback); STACK_LANES]);
    }

    /// Per-lane variant: one feedback amount (`[0.0, 7.0]`) per lane. Used by
    /// the engine when the matrix `Feedback` dest carries per-lane modulation
    /// (e.g. VoiceSpread → Feedback giving each unison lane its own growl).
    #[inline]
    pub fn set_feedback_live_lanes(&mut self, feedback: &[f32; STACK_LANES]) {
        let mut scales = [0.0_f32; STACK_LANES];
        for k in 0..STACK_LANES {
            scales[k] = fb_scale(feedback[k]);
        }
        self.write_fb_scales(scales);
    }

    #[inline]
    fn write_fb_scales(&mut self, scales: [f32; STACK_LANES]) {
        let fb_op = spec_of(self.algo).structural_fb_op;
        for i in 0..N_OPS {
            self.ops[i].fb_scale = if (i + 1) as u8 == fb_op {
                scales
            } else {
                [0.0; STACK_LANES]
            };
        }
    }

    /// Note-on: populate per-lane offsets from `stack_params`, recook every op
    /// against `voice_params`, reset phase to lane-decorrelated offsets,
    /// trigger EG attack on every op.
    ///
    /// `rng_counter` is a monotonic per-allocation counter — combined with
    /// (note, velocity) into the seed for deterministic offline rendering.
    pub fn note_on(
        &mut self,
        stack_params: &StackParams,
        voice_params: &VoiceParams,
        note: u8,
        velocity: u8,
        sample_rate: f32,
        rng_counter: u64,
    ) {
        self.note = note;
        self.velocity = velocity;
        self.gate = true;
        self.density = stack_params.density.clamp(1, STACK_LANES as u8);
        self.algo = voice_params.algo;
        self.route_fn = resolve_lane_route(voice_params.algo);

        let seed = stack_seed(note, velocity, rng_counter);
        let mut rng = seed;
        self.fill_lane_meta(stack_params, &mut rng);
        // LFO2: reseed per-lane S&H from the stack seed, then handle the
        // KeySync vs Free retrigger policy. Free preserves per-lane phases
        // across notes (still a per-instance accumulator).
        self.lfo2.reseed(seed);
        self.lfo2.note_on(&voice_params.lfo2);

        let master_mult = 2_f32.powf(voice_params.master_tune_cents / 1200.0);
        for i in 0..N_OPS {
            self.cook_op(
                i,
                &voice_params.ops[i],
                note,
                velocity,
                sample_rate,
                master_mult,
                stack_params.detune_cents_max,
            );
            self.ops[i].eg.note_on();
        }
        self.set_feedback_live(voice_params.feedback);
        self.pitch_eg
            .cook(&voice_params.pitch_eg, voice_params.peg_depth, 1.0);
        self.pitch_eg.note_on();
        self.mod_env.cook(&voice_params.mod_env);
        self.mod_env.note_on();
        self.glide_st = 0.0;
        self.apply_pitch_mult();
        self.apply_phase_offsets(stack_params.phase);
        // Cache pan inputs so `refresh_pan_with_mod` can re-run the equal-
        // power curve every block without a fresh params handle. `cached_spread`
        // no longer affects panning directly; it's the gain on the matrix's
        // `VoiceSpread` source — see `eval_sources`.
        self.cached_spread = stack_params.spread;
        for i in 0..N_OPS {
            self.cached_op_pans[i] = voice_params.ops[i].pan;
        }
        self.recompute_pan(&voice_params.ops);
        self.prev_outs = [[0.0_f32; STACK_LANES]; N_OPS];
        // Matrix writes block-rate; until the first block runs, hold zero.
        self.op_level_mod = [[0.0_f32; STACK_LANES]; N_OPS];
        self.global_pitch_mod_st = [0.0_f32; STACK_LANES];
        self.op_pitch_mod_st = [[0.0_f32; STACK_LANES]; N_OPS];
        self.op_pan_mod = [[0.0_f32; STACK_LANES]; N_OPS];
    }

    /// Solo-legato retarget: re-cook EG targets/rates and per-lane phase
    /// increments for the new pitch without resetting phase or restarting
    /// the EG. Lane spread metadata is preserved.
    pub fn retarget_pitch(
        &mut self,
        stack_params: &StackParams,
        voice_params: &VoiceParams,
        note: u8,
        velocity: u8,
        sample_rate: f32,
    ) {
        self.note = note;
        self.velocity = velocity;
        let master_mult = 2_f32.powf(voice_params.master_tune_cents / 1200.0);
        for i in 0..N_OPS {
            self.cook_op(
                i,
                &voice_params.ops[i],
                note,
                velocity,
                sample_rate,
                master_mult,
                stack_params.detune_cents_max,
            );
        }
        self.apply_pitch_mult();
    }

    pub fn note_off(&mut self) {
        self.gate = false;
        for op in &mut self.ops {
            op.eg.note_off();
        }
        self.pitch_eg.note_off();
        self.mod_env.note_off();
    }

    #[inline]
    pub fn is_idle(&self) -> bool {
        self.ops.iter().all(|o| o.eg.stage == EgStage::Idle)
    }

    #[inline]
    pub fn set_bend(&mut self, semitones: f32) {
        self.bend_st = semitones;
        self.apply_pitch_mult();
    }

    #[inline]
    pub fn set_glide(&mut self, semitones: f32) {
        self.glide_st = semitones;
        self.apply_pitch_mult();
    }

    /// Recompute per-lane `phase_inc` from `base_phase_inc` × the current
    /// pitch sum: bend + glide + `pitch_eg.level_st` + matrix `GlobalPitch` +
    /// matrix `OpNRatio`/`OpNDetune` (all semitones). Call after mutating
    /// bend / glide, or once per control tick after [`Self::eg_tick`] so the
    /// per-sample loop sees the freshly ticked Pitch EG offset plus the
    /// freshly evaluated matrix block.
    #[inline]
    pub fn apply_pitch_mult(&mut self) {
        let base_st = self.bend_st + self.glide_st + self.pitch_eg.level_st;
        for i in 0..N_OPS {
            for k in 0..STACK_LANES {
                let st = base_st + self.global_pitch_mod_st[k] + self.op_pitch_mod_st[i][k];
                let mult = 2_f32.powf(st / 12.0) as f64;
                self.ops[i].phase_inc[k] =
                    (self.ops[i].base_phase_inc[k] as f64 * mult) as u32;
            }
        }
    }

    /// Re-run the equal-power pan curve using base op pans + the matrix
    /// `OpNPan` per-lane offset. Called by the engine each block after the
    /// matrix eval so a pan-routed slot moves the lane gains without
    /// re-deriving `voice_spread` or `density`.
    ///
    /// Stack-spread no longer auto-pans lanes — wire `VoiceSpread → OpNPan`
    /// through the matrix when you want it. `cached_spread` survives because
    /// the matrix's `VoiceSpread` source multiplies the raw lane position by
    /// it, so the spread macro still gates how wide the matrix sees the
    /// lanes.
    pub fn refresh_pan_with_mod(&mut self) {
        let spec = spec_of(self.algo);
        let density = self.density as usize;
        let inv_sqrt_density = 1.0 / (density.max(1) as f32).sqrt();
        for i in 0..N_OPS {
            let is_carrier = (spec.carriers >> i) & 1 == 1;
            let op_pan = self.cached_op_pans[i];
            for k in 0..STACK_LANES {
                let active = is_carrier && k < density;
                if active {
                    let total = (op_pan + self.op_pan_mod[i][k]).clamp(-1.0, 1.0);
                    let theta = (total + 1.0) * core::f32::consts::FRAC_PI_4;
                    let (s, c) = theta.sin_cos();
                    self.pan_l[i][k] = c * inv_sqrt_density;
                    self.pan_r[i][k] = s * inv_sqrt_density;
                } else {
                    self.pan_l[i][k] = 0.0;
                    self.pan_r[i][k] = 0.0;
                }
            }
        }
    }

    /// Advance every op's EG + the patch envelopes (Pitch EG + Mod Env) one
    /// control tick (typically once per block). Re-applies the pitch mult so
    /// the per-sample loop picks up the freshly ticked Pitch EG offset.
    #[inline]
    pub fn eg_tick(&mut self, dt: f32) {
        for op in &mut self.ops {
            op.eg.tick(dt);
        }
        self.pitch_eg.tick(dt);
        self.mod_env.tick(dt);
        self.apply_pitch_mult();
    }

    /// Force every op into Sustain at `level` — fixture for steady-state
    /// tests and benches. Skips Attack/Decay.
    pub fn force_sustain(&mut self, level: f32) {
        for op in &mut self.ops {
            op.eg.stage = EgStage::Sustain;
            op.eg.level = level;
        }
    }

    // --- internals ---------------------------------------------------------

    fn fill_lane_meta(&mut self, sp: &StackParams, rng: &mut u64) {
        let density = self.density as usize;
        // Always populate all 8 lanes; trailing lanes get neutral values and
        // are silenced by the pan mask.
        for k in 0..STACK_LANES {
            self.voice_idx[k] = if k < density { k as u8 } else { 0 };
            self.voice_rand[k] = xorshift_f32(rng);
        }
        match sp.distrib {
            StackDistrib::Linear => fill_linear(&mut self.voice_spread, density),
            StackDistrib::Geometric => fill_geometric(&mut self.voice_spread, density),
            StackDistrib::Random => {
                for k in 0..STACK_LANES {
                    self.voice_spread[k] = if k < density {
                        -1.0 + 2.0 * xorshift_f32(rng)
                    } else {
                        0.0
                    };
                }
            }
        }
    }

    fn cook_op(
        &mut self,
        i: usize,
        params: &OpParams,
        key: u8,
        velocity: u8,
        sample_rate: f32,
        master_mult: f32,
        detune_cents_max: f32,
    ) {
        let base_hz = match params.ratio_mode {
            RatioMode::Ratio => {
                let num_eff = params.num as f32 + (params.fine as f32) * 0.01;
                let denom = params.denom.max(1) as f32;
                let cents = params.detune as f32;
                midi_to_hz(key) * (num_eff / denom) * 2_f32.powf(cents / 1200.0)
            }
            RatioMode::Fixed => params.fixed_hz,
        };
        let base_inc = ((base_hz * master_mult / sample_rate) * PM_SCALE_Q32) as f64;
        for k in 0..STACK_LANES {
            let lane_cents = detune_cents_max * self.voice_spread[k];
            let lane_factor = 2_f64.powf(lane_cents as f64 / 1200.0);
            self.ops[i].base_phase_inc[k] = (base_inc * lane_factor) as u32;
        }

        let ks_lvl = ks_level_mult(
            key,
            params.ks_break_pt,
            params.ks_l_depth,
            params.ks_l_curve,
            params.ks_r_depth,
            params.ks_r_curve,
        );
        let vel = vel_factor(params.vel_sens, velocity);
        let level_norm = (params.level.min(99) as f32) / 99.0;
        let max_amp = level_norm * ks_lvl * vel;
        let rate_mult = ks_rate_mult(key, params.ks_rate);
        self.ops[i].eg.cook(&params.eg, max_amp, rate_mult);

        // Feedback is no longer per-op: see `set_feedback_live`. cook_op
        // leaves `fb_scale` alone; note_on calls the live setter after the
        // cook loop, and the engine refreshes it each block.
    }

    fn apply_phase_offsets(&mut self, phase_amount: f32) {
        // Per-lane Q32 offset shared across all six ops at note-on.
        let mut lane_offset = [0u32; STACK_LANES];
        for k in 0..STACK_LANES {
            let frac = (phase_amount * self.voice_rand[k]).clamp(0.0, 1.0);
            lane_offset[k] = (frac as f64 * PM_SCALE_Q32 as f64) as u32;
        }
        for i in 0..N_OPS {
            for k in 0..STACK_LANES {
                self.ops[i].phase[k] = lane_offset[k];
                self.ops[i].fb_prev1[k] = 0.0;
                self.ops[i].fb_prev2[k] = 0.0;
            }
        }
    }

    fn recompute_pan(&mut self, op_params: &[OpParams; N_OPS]) {
        let spec = spec_of(self.algo);
        let density = self.density as usize;
        // Decorrelated lanes sum to ~√N amplitude. Bake 1/√density into the
        // pan tables so density 1..8 are level-matched without a per-sample
        // multiply.
        let inv_sqrt_density = 1.0 / (density.max(1) as f32).sqrt();
        for i in 0..N_OPS {
            let is_carrier = (spec.carriers >> i) & 1 == 1;
            let op_pan = op_params[i].pan;
            for k in 0..STACK_LANES {
                let active = is_carrier && k < density;
                if active {
                    let total = op_pan.clamp(-1.0, 1.0);
                    let theta = (total + 1.0) * core::f32::consts::FRAC_PI_4;
                    let (s, c) = theta.sin_cos();
                    self.pan_l[i][k] = c * inv_sqrt_density;
                    self.pan_r[i][k] = s * inv_sqrt_density;
                } else {
                    self.pan_l[i][k] = 0.0;
                    self.pan_r[i][k] = 0.0;
                }
            }
        }
    }
}

fn fill_linear(out: &mut [f32; STACK_LANES], density: usize) {
    if density <= 1 {
        for k in 0..STACK_LANES {
            out[k] = 0.0;
        }
        return;
    }
    let denom = (density - 1) as f32;
    for k in 0..STACK_LANES {
        out[k] = if k < density {
            -1.0 + 2.0 * (k as f32) / denom
        } else {
            0.0
        };
    }
}

fn fill_geometric(out: &mut [f32; STACK_LANES], density: usize) {
    // Same anchor points as Linear but `sign(t) * |t|^0.5` — pushes inner
    // lanes toward 0 and outer lanes closer to ±1 (denser at the edges).
    fill_linear(out, density);
    for k in 0..STACK_LANES {
        let t = out[k];
        let mag = t.abs().sqrt();
        out[k] = if t < 0.0 { -mag } else { mag };
    }
}

/// Per-sample stereo tick. Routes prev-sample outputs into mod inputs, ticks
/// every op across all 8 lanes, folds carriers into `(L, R)` using the
/// precomputed pan matrix.
#[inline]
pub fn stack_tick_stereo(stack: &mut Stack) -> (f32, f32) {
    let (mi, _cs) = (stack.route_fn)(&stack.prev_outs);
    let mut new_outs = [[0.0_f32; STACK_LANES]; N_OPS];
    for i in 0..N_OPS {
        let lvl_mod = stack.op_level_mod[i];
        let op = &mut stack.ops[i];
        let lvl = op.eg.level;
        let fbs = op.fb_scale;
        // Stage 1: phase-modulation Q32 per lane.
        let mut pm_q32 = [0u32; STACK_LANES];
        for k in 0..STACK_LANES {
            let fb_avg = 0.5 * (op.fb_prev1[k] + op.fb_prev2[k]);
            let total_mod = mi[i][k] + fb_avg * fbs[k];
            pm_q32[k] = (total_mod * PM_SCALE_Q32) as i32 as u32;
        }
        // Stage 2: read sine at modulated phase, scale by EG level plus the
        // mod-matrix per-lane offset (clamped — additive on a [0,1] base).
        let mut sines = [0.0_f32; STACK_LANES];
        for k in 0..STACK_LANES {
            let phase_mod = op.phase[k].wrapping_add(pm_q32[k]);
            let lvl_k = (lvl + lvl_mod[k]).clamp(0.0, 1.0);
            sines[k] = fast_sine_q32(phase_mod) * lvl_k;
        }
        // Stage 3: advance phase + rotate feedback memory.
        for k in 0..STACK_LANES {
            new_outs[i][k] = sines[k];
            op.phase[k] = op.phase[k].wrapping_add(op.phase_inc[k]);
        }
        op.fb_prev2 = op.fb_prev1;
        op.fb_prev1 = sines;
    }
    // Stereo fold from prev_outs (1-sample-delay convention, matches algo.rs).
    let mut l = 0.0_f32;
    let mut r = 0.0_f32;
    for i in 0..N_OPS {
        for k in 0..STACK_LANES {
            l += stack.prev_outs[i][k] * stack.pan_l[i][k];
            r += stack.prev_outs[i][k] * stack.pan_r[i][k];
        }
    }
    stack.prev_outs = new_outs;
    (l, r)
}

/// Per-sample mono tick. Sums every carrier across active lanes — no pan,
/// no stereo. Used by silence-detection paths and benches that don't care
/// about stereo placement.
#[inline]
pub fn stack_tick_mono(stack: &mut Stack) -> f32 {
    let (mi, _cs_lane) = (stack.route_fn)(&stack.prev_outs);
    let mut new_outs = [[0.0_f32; STACK_LANES]; N_OPS];
    for i in 0..N_OPS {
        let lvl_mod = stack.op_level_mod[i];
        let op = &mut stack.ops[i];
        let lvl = op.eg.level;
        let fbs = op.fb_scale;
        let mut pm_q32 = [0u32; STACK_LANES];
        for k in 0..STACK_LANES {
            let fb_avg = 0.5 * (op.fb_prev1[k] + op.fb_prev2[k]);
            let total_mod = mi[i][k] + fb_avg * fbs[k];
            pm_q32[k] = (total_mod * PM_SCALE_Q32) as i32 as u32;
        }
        let mut sines = [0.0_f32; STACK_LANES];
        for k in 0..STACK_LANES {
            let phase_mod = op.phase[k].wrapping_add(pm_q32[k]);
            let lvl_k = (lvl + lvl_mod[k]).clamp(0.0, 1.0);
            sines[k] = fast_sine_q32(phase_mod) * lvl_k;
        }
        for k in 0..STACK_LANES {
            new_outs[i][k] = sines[k];
            op.phase[k] = op.phase[k].wrapping_add(op.phase_inc[k]);
        }
        op.fb_prev2 = op.fb_prev1;
        op.fb_prev1 = sines;
    }
    let spec = spec_of(stack.algo);
    let density = stack.density as usize;
    let mut sum = 0.0_f32;
    for i in 0..N_OPS {
        if (spec.carriers >> i) & 1 == 1 {
            for k in 0..density {
                sum += stack.prev_outs[i][k];
            }
        }
    }
    sum *= 1.0 / (density.max(1) as f32).sqrt();
    stack.prev_outs = new_outs;
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algo::N_OPS;
    use crate::op::OpParams;

    fn carrier_friendly_patch() -> VoiceParams {
        let mut ops = [OpParams::default(); N_OPS];
        for op in &mut ops {
            op.eg.r[3] = 99;
        }
        VoiceParams {
            ops,
            algo: 32,
            ..VoiceParams::default()
        }
    }

    #[test]
    fn density_1_silences_lanes_1_through_7() {
        let mut stack = Stack::default();
        let sp = StackParams {
            density: 1,
            ..StackParams::default()
        };
        let vp = carrier_friendly_patch();
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        for i in 0..N_OPS {
            for k in 1..STACK_LANES {
                assert_eq!(stack.pan_l[i][k], 0.0);
                assert_eq!(stack.pan_r[i][k], 0.0);
            }
        }
    }

    #[test]
    fn density_lane_meta_matches_distrib() {
        let mut stack = Stack::default();
        let sp = StackParams {
            density: 5,
            detune_cents_max: 10.0,
            spread: 0.5,
            phase: 0.0,
            distrib: StackDistrib::Linear,
        };
        let vp = carrier_friendly_patch();
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 42);
        // Active lanes: spread −1, −0.5, 0, +0.5, +1.
        let expected = [-1.0, -0.5, 0.0, 0.5, 1.0];
        for (k, want) in expected.iter().enumerate() {
            assert!(
                (stack.voice_spread[k] - want).abs() < 1e-6,
                "lane {k} spread = {} want {want}",
                stack.voice_spread[k]
            );
        }
    }

    #[test]
    fn fresh_stack_is_idle() {
        let stack = Stack::default();
        assert!(stack.is_idle());
    }

    #[test]
    fn note_on_attacks_all_ops() {
        let mut stack = Stack::default();
        let sp = StackParams::default();
        let vp = carrier_friendly_patch();
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        assert!(stack.gate);
        for op in &stack.ops {
            assert_eq!(op.eg.stage, EgStage::Attack);
        }
    }

    #[test]
    fn note_off_to_idle_with_fast_release() {
        let mut stack = Stack::default();
        let sp = StackParams::default();
        let vp = carrier_friendly_patch();
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        let dt = 1.0 / 48_000.0;
        for _ in 0..(48_000 / 4) {
            stack.eg_tick(dt);
        }
        stack.note_off();
        let mut steps = 0;
        while !stack.is_idle() && steps < 48_000 {
            stack.eg_tick(dt);
            steps += 1;
        }
        assert!(stack.is_idle(), "stack never went idle");
    }

    #[test]
    fn stack_tick_finite_for_density_1_4_8() {
        let vp = carrier_friendly_patch();
        for &density in &[1u8, 4, 8] {
            let mut stack = Stack::default();
            let sp = StackParams {
                density,
                ..StackParams::default()
            };
            stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
            stack.eg_tick(64.0 / 48_000.0);
            for _ in 0..256 {
                let (l, r) = stack_tick_stereo(&mut stack);
                assert!(l.is_finite() && r.is_finite(), "density {density} non-finite");
            }
        }
    }

    #[test]
    fn density_1_stereo_matches_centre_pan() {
        let mut stack = Stack::default();
        let sp = StackParams {
            density: 1,
            spread: 0.5,
            phase: 0.0,
            ..StackParams::default()
        };
        let vp = carrier_friendly_patch();
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        for op in &mut stack.ops {
            op.eg.stage = EgStage::Sustain;
            op.eg.level = 0.4;
        }
        for _ in 0..8 {
            stack_tick_stereo(&mut stack);
        }
        let (l, r) = stack_tick_stereo(&mut stack);
        assert!(
            (l - r).abs() < 1e-5,
            "density 1 + spread should be centre, L={l} R={r}"
        );
    }

    #[test]
    fn stack_spread_does_not_auto_pan_without_matrix() {
        // Auto pan-spread was dropped — `stack-spread` × `voice_spread[k]` is
        // no longer baked into `pan_l/pan_r`. With no matrix `VoiceSpread →
        // OpNPan` slot active, every lane sits at the op's base pan (centre
        // here), regardless of the spread macro.
        let mut stack = Stack::default();
        let sp = StackParams {
            density: 2,
            detune_cents_max: 0.0,
            spread: 1.0,
            phase: 0.0,
            distrib: StackDistrib::Linear,
        };
        let vp = carrier_friendly_patch();
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        // Both lanes centred → pan_l == pan_r per lane, scaled by 1/√density.
        let centre = (core::f32::consts::FRAC_PI_4).cos() / 2.0_f32.sqrt();
        for k in 0..2 {
            assert!(
                (stack.pan_l[0][k] - centre).abs() < 1e-4,
                "lane {k} pan_l {} not centred (want {centre})",
                stack.pan_l[0][k],
            );
            assert!(
                (stack.pan_r[0][k] - centre).abs() < 1e-4,
                "lane {k} pan_r {} not centred (want {centre})",
                stack.pan_r[0][k],
            );
        }
        // `cached_spread` is still captured — the matrix uses it to scale the
        // `VoiceSpread` source.
        assert!((stack.cached_spread - 1.0).abs() < 1e-6);
    }

    #[test]
    fn detune_writes_distinct_phase_increments() {
        let mut stack = Stack::default();
        let sp = StackParams {
            density: 4,
            detune_cents_max: 50.0,
            spread: 0.0,
            phase: 0.0,
            distrib: StackDistrib::Linear,
        };
        let vp = carrier_friendly_patch();
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        // Lane 0 (spread −1) and lane 3 (spread +1) should differ in inc.
        assert_ne!(
            stack.ops[0].phase_inc[0],
            stack.ops[0].phase_inc[3],
            "detune produced identical lane increments"
        );
    }

    #[test]
    fn phase_decorrelation_writes_distinct_phases() {
        let mut stack = Stack::default();
        let sp = StackParams {
            density: 4,
            detune_cents_max: 0.0,
            spread: 0.0,
            phase: 1.0,
            distrib: StackDistrib::Linear,
        };
        let vp = carrier_friendly_patch();
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        // Lanes should have different starting phases for op 0.
        let mut distinct = std::collections::HashSet::new();
        for k in 0..4 {
            distinct.insert(stack.ops[0].phase[k]);
        }
        assert!(
            distinct.len() >= 2,
            "phase decorrelation produced colliding lane phases"
        );
    }

    #[test]
    fn rng_is_deterministic_across_seeds() {
        let vp = carrier_friendly_patch();
        let sp = StackParams::default();
        let mut a = Stack::default();
        let mut b = Stack::default();
        a.note_on(&sp, &vp, 60, 100, 48_000.0, 7);
        b.note_on(&sp, &vp, 60, 100, 48_000.0, 7);
        assert_eq!(a.voice_rand, b.voice_rand);
    }

    #[test]
    fn rng_changes_with_counter() {
        let vp = carrier_friendly_patch();
        let sp = StackParams::default();
        let mut a = Stack::default();
        let mut b = Stack::default();
        a.note_on(&sp, &vp, 60, 100, 48_000.0, 1);
        b.note_on(&sp, &vp, 60, 100, 48_000.0, 2);
        assert_ne!(a.voice_rand, b.voice_rand);
    }

    #[test]
    fn bend_scales_all_lane_increments() {
        let mut stack = Stack::default();
        let sp = StackParams {
            density: 4,
            ..StackParams::default()
        };
        let vp = carrier_friendly_patch();
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        let baseline: Vec<u32> = (0..4).map(|k| stack.ops[0].phase_inc[k]).collect();
        stack.set_bend(12.0); // +1 octave
        for k in 0..4 {
            let want = (baseline[k] as u64 * 2).min(u32::MAX as u64) as u32;
            let got = stack.ops[0].phase_inc[k];
            // Within 1 part in 1e4 for f64 rounding.
            let ratio = got as f64 / baseline[k] as f64;
            assert!(
                (ratio - 2.0).abs() < 1e-4,
                "lane {k}: bend ratio {ratio}, want ≈2 (got {got} / base {})",
                baseline[k]
            );
            let _ = want;
        }
    }

    #[test]
    fn fresh_stack_silent() {
        let mut stack = Stack::default();
        let mut peak = 0.0_f32;
        for _ in 0..256 {
            let (l, r) = stack_tick_stereo(&mut stack);
            let m = l.abs().max(r.abs());
            if m > peak {
                peak = m;
            }
        }
        assert_eq!(peak, 0.0);
    }

    #[test]
    fn density_8_produces_audible_output() {
        let mut stack = Stack::default();
        let sp = StackParams {
            density: 8,
            ..StackParams::default()
        };
        let vp = carrier_friendly_patch();
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        for op in &mut stack.ops {
            op.eg.stage = EgStage::Sustain;
            op.eg.level = 0.4;
        }
        let mut peak = 0.0_f32;
        for _ in 0..512 {
            let (l, r) = stack_tick_stereo(&mut stack);
            let m = l.abs().max(r.abs());
            if m > peak {
                peak = m;
            }
        }
        assert!(peak > 0.1, "density 8 produced silent stereo, peak={peak}");
    }

    #[test]
    fn density_0_clamps_to_1() {
        let mut stack = Stack::default();
        let sp = StackParams {
            density: 0,
            ..StackParams::default()
        };
        let vp = carrier_friendly_patch();
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        assert_eq!(stack.density, 1);
    }

    #[test]
    fn density_9_clamps_to_8() {
        let mut stack = Stack::default();
        let sp = StackParams {
            density: 9,
            ..StackParams::default()
        };
        let vp = carrier_friendly_patch();
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        assert_eq!(stack.density, STACK_LANES as u8);
    }

    #[test]
    fn lane_route_matches_scalar_for_all_algos() {
        // Per-algo: a ping on lane 0 of each op should propagate identically
        // to the scalar router for that lane.
        for algo in 1..=N_ALGOS as u8 {
            let lane_fn = resolve_lane_route(algo);
            let scalar_fn = crate::algo::resolve_route(algo);
            for op in 0..N_OPS {
                let mut prev_lane = [[0.0_f32; STACK_LANES]; N_OPS];
                let mut prev_scalar = [0.0_f32; N_OPS];
                prev_lane[op][0] = 1.0;
                prev_scalar[op] = 1.0;
                let (mi_lane, cs_lane) = lane_fn(&prev_lane);
                let (mi_scalar, cs_scalar) = scalar_fn(&prev_scalar);
                assert_eq!(cs_lane[0], cs_scalar, "algo {algo} op {op} cs mismatch");
                for j in 0..N_OPS {
                    assert_eq!(
                        mi_lane[j][0], mi_scalar[j],
                        "algo {algo} op {op}→{j} mi mismatch"
                    );
                }
            }
        }
    }

    // --- ticket 0007: Pitch EG + Mod Env ----------------------------------

    #[test]
    fn pitch_eg_lifts_phase_inc_then_settles() {
        // AC: Pitch EG L1=+50, L2/L3/L4=0, R1/R2 fast — held note sweeps up
        // and back to centre. Observe via per-lane `phase_inc`: it should
        // peak above the baseline soon after note-on then return to baseline.
        let mut stack = Stack::default();
        let sp = StackParams {
            density: 1,
            detune_cents_max: 0.0,
            spread: 0.0,
            phase: 0.0,
            distrib: StackDistrib::Linear,
        };
        let mut vp = carrier_friendly_patch();
        vp.pitch_eg = crate::envelope::PitchEgParams {
            r: [99, 99, 99, 99],
            l: [50, 0, 0, 0],
        };
        vp.peg_depth = 1.0;
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        let baseline = stack.ops[0].base_phase_inc[0];
        // Immediately after note-on the PEG hasn't moved yet — phase_inc =
        // baseline modulo f64 rounding.
        assert!(
            (stack.ops[0].phase_inc[0] as i64 - baseline as i64).abs() <= 2,
            "initial phase_inc {} not ≈baseline {baseline}",
            stack.ops[0].phase_inc[0]
        );

        let dt = 64.0 / 48_000.0;
        let mut peak_inc: u32 = 0;
        let mut reached_decay1 = false;
        let mut settled_at = None;
        for i in 0..4000 {
            stack.eg_tick(dt);
            let inc = stack.ops[0].phase_inc[0];
            if inc > peak_inc {
                peak_inc = inc;
            }
            if stack.pitch_eg.stage == crate::envelope::EnvStage4::Decay1
                || stack.pitch_eg.stage == crate::envelope::EnvStage4::Decay2
            {
                reached_decay1 = true;
            }
            if reached_decay1 && stack.pitch_eg.level_st.abs() < 0.005 {
                settled_at = Some(i);
                break;
            }
        }
        // Peak should be measurably above baseline (≈+0.505 semitones ≈
        // 2.96% increase in phase_inc).
        let lift = (peak_inc as f64 / baseline as f64) - 1.0;
        assert!(
            lift > 0.02,
            "peak lift {lift} too low (baseline {baseline}, peak {peak_inc})"
        );
        assert!(settled_at.is_some(), "PEG never returned to centre");
        // After settling, phase_inc back near baseline.
        let final_inc = stack.ops[0].phase_inc[0];
        let final_delta = (final_inc as i64 - baseline as i64).abs();
        assert!(
            final_delta < (baseline as i64) / 200,
            "final phase_inc {final_inc} not ≈baseline {baseline}"
        );
    }

    #[test]
    fn mod_env_runs_through_adsr_on_note_lifecycle() {
        // Mod Env reaches sustain after note-on, releases to idle after
        // note-off.
        let mut stack = Stack::default();
        let sp = StackParams::default();
        let mut vp = carrier_friendly_patch();
        vp.mod_env = crate::envelope::ModEnvParams {
            a_ms: 5.0,
            d_ms: 10.0,
            s: 0.4,
            r_ms: 10.0,
            shape: crate::envelope::AdsrShape::Lin,
        };
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        let dt = 64.0 / 48_000.0;
        for _ in 0..2000 {
            stack.eg_tick(dt);
            if stack.mod_env.stage == crate::envelope::AdsrStage::Sustain {
                break;
            }
        }
        assert_eq!(
            stack.mod_env.stage,
            crate::envelope::AdsrStage::Sustain,
            "mod env never reached sustain"
        );
        assert!(
            (stack.mod_env.level - 0.4).abs() < 1e-2,
            "sustain {} want ≈0.4",
            stack.mod_env.level
        );
        stack.note_off();
        for _ in 0..2000 {
            stack.eg_tick(dt);
            if stack.mod_env.stage == crate::envelope::AdsrStage::Idle {
                break;
            }
        }
        assert_eq!(
            stack.mod_env.stage,
            crate::envelope::AdsrStage::Idle,
            "mod env never returned to idle"
        );
    }

    #[test]
    fn default_envelopes_dont_perturb_pitch() {
        // PitchEgParams default → all levels 0 → tick should leave
        // phase_inc identical to baseline within rounding.
        let mut stack = Stack::default();
        let sp = StackParams::default();
        let vp = carrier_friendly_patch(); // default PitchEgParams + peg_depth=1.0
        stack.note_on(&sp, &vp, 60, 100, 48_000.0, 0);
        let baseline = stack.ops[0].base_phase_inc[0];
        let dt = 64.0 / 48_000.0;
        for _ in 0..500 {
            stack.eg_tick(dt);
            let inc = stack.ops[0].phase_inc[0];
            assert!(
                (inc as i64 - baseline as i64).abs() <= 4,
                "default PEG perturbed pitch: inc {inc} baseline {baseline}"
            );
        }
    }
}
